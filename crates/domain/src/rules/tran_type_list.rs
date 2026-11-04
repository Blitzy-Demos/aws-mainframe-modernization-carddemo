//! `COTRTLIC` (CTLI): the list of the Db2 transaction type master with its
//! type and description filters, the inline description update of one listed
//! row and the delete of one listed row
//! [app/app-transaction-type-db2/cbl/COTRTLIC.cbl].
//!
//! The rules work on the Db2 master `CARDDEMO.TRANSACTION_TYPE` (table
//! `transaction_types`) through [`TranTypeMasterStore`] only. The reports read
//! the VSAM processing copy, which an edit made here reaches only through
//! `batch seed --only tranextr`.
//!
//! The REST operations are those of
//! `crates/api/src/handlers/admin_transaction_types.rs`:
//!
//! - [`list`]: `GET /admin/transaction-types` with the filters and the F7/F8
//!   browse (`1200-EDIT-INPUTS`, `8000-READ-FORWARD`, `8100-READ-BACKWARDS`);
//! - [`update_description`]: `PUT /admin/transaction-types/{id}?origin=list`,
//!   the `U` selector and its F10 save (`1211-EDIT-ARRAY-DESC`,
//!   `9200-UPDATE-RECORD`); without `origin` the `COTRTUPC` rules of
//!   `super::tran_type_update` apply instead;
//! - [`delete`]: `DELETE /admin/transaction-types/{id}`, the `D` selector and
//!   its F10 confirmation (`9300-DELETE-RECORD`).
//!
//! # The pseudo-conversation
//!
//! `COTRTLIC` keeps its page state in the COMMAREA: the filters, the seven
//! rows shown, the first and last codes, the page number and the pending row
//! action (P-COTRTLIC-011). The stateless API carries the filters, the start
//! key and the direction in every request, and the row actions travel in
//! requests of their own, so the selector edits of `1210-EDIT-ARRAY` have no
//! input here (E-54). Where a COBOL outcome depends on the page number, the
//! rules judge it from the data instead: the page a request reaches is the
//! first page when no type that passes the filters precedes it.
//!
//! # Db2 texts
//!
//! `9999-FORMAT-DB2-MESSAGE` strings the action text, the SQLCODE edited
//! `----9` and the `DSNTIAC` text ([`tran_type_list_db2_message`]). The
//! SQLCODE is the Db2 code of the condition the store reports
//! ([`PortError::sqlcode`]). The `DSNTIAC` slot stays empty: escalation E-30
//! gives it the database's own message text, which no [`PortError`] carries.
//! A failure without a Db2 code ([`PortError::Unavailable`]) answers the
//! unexpected-error text, because its text is adapter detail that never
//! reaches a response.
//!
//! A failed cursor read names the step of the cursor that failed, its `OPEN`,
//! a `FETCH` or its `CLOSE` ([`CursorPhase`]), and the action text is that
//! step's, as the program moves it.

use std::iter::repeat_n;

use crate::error::{DomainError, ErrorKind};
use crate::messages::{
    C_TR_TYPE_BACKWARD_CLOSE, C_TR_TYPE_BACKWARD_OPEN, C_TR_TYPE_FORWARD_CLOSE,
    C_TR_TYPE_FORWARD_FETCH, C_TR_TYPE_FORWARD_OPEN, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    DEADLOCK_SOMEONE_ELSE_UPDATING, DELETE_FAILED_WITH_MESSAGE,
    ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD, FIELD_TRANSACTION_DESC,
    HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE, HIGHLIGHTED_ROW_WAS_UPDATED, MUST_BE_SUPPLIED,
    NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES, NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS,
    NO_MORE_PAGES_TO_DISPLAY_COTRTLIC, NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC,
    NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS,
    NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC,
    PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST, RECORD_NOT_FOUND_DELETED_BY_OTHERS,
    TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT, UNEXPECTED_ERROR, UPDATE_FAILED_WITH,
    field_edit_message, tran_type_list_db2_message,
};
use crate::numeric::TranTypeCode;
use crate::ports::{
    Browse, CursorError, CursorPhase, CursorResult, Direction, PortError, TranTypeFilter,
    TranTypeMasterStore,
};

use super::ReturnMsg;

/// The transaction type row [`list`] pages through, re-exported from
/// [`crate::entities`] for the API adapter, which maps it to its response.
pub use crate::entities::TranTypeMaster;

/// The rows of one list page: `WS-MAX-SCREEN-LINES PIC S9(4) COMP VALUE 7`
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L60] (P-COTRTLIC-003).
pub const SCREEN_ROWS: u16 = 7;

/// The BMS field of the type code filter, `TRTYPEI PIC X(2)`
/// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L66].
const FIELD_TYPE_FILTER: &str = "TRTYPE";

/// The BMS field of the description filter, `TRDESCI PIC X(50)`
/// [app/app-transaction-type-db2/cpy-bms/COTRTLI.cpy:L72].
const FIELD_DESC_FILTER: &str = "TRDESC";

/// The width of the description fields the program receives: the filter
/// `TRDESCI` and its `WS-IN-TYPE-DESC`, and the row field `TRTYPDI` and its
/// `WS-ROW-TR-DESC-IN`, all `PIC X(50)`
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L172]
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L313].
const DESCRIPTION_FIELD_LEN: usize = 50;

/// `ZEROS` in the `X(02)` type code filter, which `1220-EDIT-TYPECD` treats as
/// no filter [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1101-1107].
const TYPE_CODE_ZEROS: &[u8; 2] = b"00";

/// `LIT-ASTERISK`, the text `2300-SCREEN-ARRAY-INIT` shows for a blank
/// description and `1100-RECEIVE-SCREEN` reads back as `LOW-VALUES`
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L55].
const LIT_ASTERISK: &str = "*";

/// The `'%'` `1230-EDIT-DESC` strings around the trimmed description filter
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1156-1162].
const LIKE_ANY: &str = "%";

/// The `DSNTIAC` text of a Db2 message. Escalation E-30 (open) gives the slot
/// the database's message text; no [`PortError`] carries one, so the slot is
/// empty and the message ends with the separator space.
const NO_DSNTIAC_TEXT: &str = "";

/// One `GET /admin/transaction-types` request: the two filters as entered and
/// the browse position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeListInput {
    /// The type code filter as entered in `TRTYPEI X(2)`, `None` when not
    /// sent.
    pub type_filter: Option<String>,
    /// The description filter as entered in `TRDESCI X(50)`, `None` when not
    /// sent.
    pub desc_filter: Option<String>,
    /// Where the browse starts: for a forward request (F8, Next) the
    /// look-ahead code a previous page returned as `next_key`, for a backward
    /// one (F7, Previous) the first code shown, as `prev_key`. `None` asks
    /// for the first page.
    pub start_key: Option<TranTypeCode>,
    /// Forward (ENTER, F8) or backward (F7).
    pub direction: Direction,
    /// The rows of a page, [`SCREEN_ROWS`] unless the request asks for
    /// another count; 0 counts as 1.
    pub page_size: u16,
}

/// An unfiltered request for the first page of [`SCREEN_ROWS`] rows.
impl Default for TranTypeListInput {
    fn default() -> Self {
        Self {
            type_filter: None,
            desc_filter: None,
            start_key: None,
            direction: Direction::Forward,
            page_size: SCREEN_ROWS,
        }
    }
}

/// One page of the list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranTypePage {
    /// The types shown, in ascending `TR_TYPE` order.
    pub items: Vec<TranTypeMaster>,
    /// The code a forward request starts at to show the next page:
    /// `WS-CA-LAST-TR-CODE` with `CA-NEXT-PAGE-EXISTS`; `None` when no page
    /// follows.
    pub next_key: Option<TranTypeCode>,
    /// The code a backward request starts at to show the previous page:
    /// `WS-CA-FIRST-TR-CODE`; `None` on the first page.
    pub prev_key: Option<TranTypeCode>,
    /// The error-line text of a page that is not an error: an empty result or
    /// a page boundary (E-54).
    pub message: Option<&'static str>,
}

/// The outcome of a stored inline description update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeUpdated {
    /// The master as stored.
    pub transaction_type: TranTypeMaster,
    /// The text of the response: the error-line text when the program set
    /// one, [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES`] for a
    /// description equal to the listed one but for case (V-COTRTLIC-012),
    /// else the info-line text.
    pub message: &'static str,
    /// The info line, [`HIGHLIGHTED_ROW_WAS_UPDATED`] (P-COTRTLIC-009).
    pub info: &'static str,
}

/// The outcome of a delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeDeleted {
    /// The type code deleted.
    pub type_code: TranTypeCode,
    /// [`HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE`] (P-COTRTLIC-007).
    pub message: &'static str,
}

/// The filters of the list: `1230-EDIT-DESC` then `1220-EDIT-TYPECD`, in the
/// order `1200-EDIT-INPUTS` performs them
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L960-976].
///
/// COTRTLIC 1220-EDIT-TYPECD, 1230-EDIT-DESC — P-COTRTLIC-003,
/// P-COTRTLIC-014, P-COTRTLIC-018, P-COTRTLIC-020, V-COTRTLIC-001
///
/// - The type code is moved into `X(2)`. Low-values, spaces or `00` give no
///   type condition; two digits give `TR_TYPE = code`; anything else gives
///   no condition and records [`TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT`]
///   on the type filter `TRTYPE`.
/// - The description is moved into `X(50)` (`receive_description`).
///   Low-values or spaces give no description condition; any other text
///   gives `TR_DESCRIPTION LIKE` the pattern `%`, the text without its
///   leading and trailing spaces, `%`. The text is not edited, so a `%` or
///   `_` in it is a wildcard too, and a character outside ASCII stays in the
///   pattern, which then matches only descriptions holding it.
///
/// `1220-EDIT-TYPECD` moves its text whatever `WS-RETURN-MSG` holds. On a
/// list request no earlier edit sets one (`1210-EDIT-ARRAY` has no input
/// here, E-54, and `1230-EDIT-DESC` sets none), so recording it in `msg` only
/// while `msg` is off gives the same first message.
#[must_use]
pub fn edit_filters(
    type_filter: Option<&str>,
    desc_filter: Option<&str>,
    msg: &mut ReturnMsg,
) -> TranTypeFilter {
    let description = desc_filter.and_then(description_pattern);
    let type_code = type_filter.and_then(|entered| edit_type_code(entered, msg));
    TranTypeFilter {
        type_code,
        description,
    }
}

/// COTRTLIC 1220-EDIT-TYPECD — P-COTRTLIC-018, V-COTRTLIC-001
fn edit_type_code(entered: &str, msg: &mut ReturnMsg) -> Option<TranTypeCode> {
    let code = TranTypeCode::move_from(entered);
    if code.as_alnum().is_blank() || code.as_bytes() == TYPE_CODE_ZEROS {
        return None;
    }
    if code.as_alnum().is_numeric() {
        return Some(code);
    }
    msg.set_if_off(
        TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT,
        FIELD_TYPE_FILTER,
    );
    None
}

/// COTRTLIC 1230-EDIT-DESC — P-COTRTLIC-003, P-COTRTLIC-020
///
/// The cursors compare with `TRIM(:WS-TYPE-DESC-FILTER)`, which drops the
/// padding of the `X(52)` host variable, so the pattern holds exactly the
/// strung bytes.
fn description_pattern(entered: &str) -> Option<String> {
    let field = receive_description(entered);
    if is_spaces_or_low_values(&field) {
        return None;
    }
    Some([LIKE_ANY, field.trim_matches(' '), LIKE_ANY].concat())
}

/// The COBOL conditions `= SPACES` and `= LOW-VALUES` of a whole field.
fn is_spaces_or_low_values(field: &str) -> bool {
    field.bytes().all(|byte| byte == b' ') || field.bytes().all(|byte| byte == 0)
}

/// COTRTLIC 1100-RECEIVE-SCREEN, 1230-EDIT-DESC, 1240-EDIT-ALPHANUM-REQD —
/// P-COTRTLIC-013, P-COTRTLIC-020, P-COTRTLIC-021
///
/// The alphanumeric `MOVE` of an entered description into its `PIC X(50)`
/// field, the filter `WS-IN-TYPE-DESC` or the row's `WS-ROW-TR-DESC-IN`:
/// left-justified, padded with spaces to 50 bytes, and cut on the right
/// after byte 50, as the COBOL cuts it.
///
/// A character outside ASCII takes two to four bytes, so the cut can fall
/// inside one. The COBOL field then keeps its leading bytes, each `0x80` or
/// more and so no letter, digit or space. That character is kept whole here,
/// standing for those bytes, so the text stays valid UTF-8 and the edits see
/// what they see in the COBOL field. In a text of at most 50 characters the
/// first character outside ASCII starts before byte 50, so that character
/// is always received: the row edit refuses it as not alphanumeric
/// (V-COTRTLIC-014), and the filter pattern holds it, so the count finds
/// only descriptions holding it (V-COTRTLIC-002 when none does). Cutting
/// before the character instead would drop it and edit, store or search the
/// shorter text before it.
fn receive_description(entered: &str) -> String {
    let mut end = entered.len().min(DESCRIPTION_FIELD_LEN);
    // The end of the text is always a boundary, so this stops.
    while !entered.is_char_boundary(end) {
        end += 1;
    }
    let mut field = String::with_capacity(end.max(DESCRIPTION_FIELD_LEN));
    field.push_str(entered.get(..end).unwrap_or_default());
    field.extend(repeat_n(' ', DESCRIPTION_FIELD_LEN.saturating_sub(end)));
    field
}

/// Lists the transaction types that pass the filters, one page from the start
/// key in the direction asked.
///
/// COTRTLIC 1200-EDIT-INPUTS, 1290-CROSS-EDITS, 8000-READ-FORWARD,
/// 8100-READ-BACKWARDS, 2500-SETUP-MESSAGE — P-COTRTLIC-003, P-COTRTLIC-004,
/// P-COTRTLIC-012, P-COTRTLIC-014, P-COTRTLIC-022, P-COTRTLIC-028,
/// P-COTRTLIC-030 to P-COTRTLIC-034, P-COTRTLIC-036 to P-COTRTLIC-040,
/// P-COTRTLIC-044, P-COTRTLIC-045, V-COTRTLIC-001, V-COTRTLIC-002,
/// V-COTRTLIC-004 to V-COTRTLIC-010, V-COTRTLIC-024 to V-COTRTLIC-028
///
/// The filters are edited first ([`edit_filters`]). When a valid filter is
/// given, the types that pass it are counted (`9100-CHECK-FILTERS`, which
/// ignores a rejected type code), and none passing rejects the request. A
/// rejected type code then rejects it; otherwise the page is read:
///
/// - forward without a start key, the first page; when it is empty its
///   message is [`NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC`];
/// - forward from a start key (F8), the page from that code inclusive:
///   [`NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS`] when no page follows it,
///   [`NO_MORE_PAGES_TO_DISPLAY_COTRTLIC`] when nothing is left from the
///   start key on;
/// - backward from a start key (F7), the page before that code, shown in
///   ascending order: when it is the first page its message is
///   [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`], and when no type precedes the
///   start key the program's F7 on the first page applies, which reads the
///   page forward from the start key with that message;
/// - backward from a start key whose read fails, the same first-page outcomes
///   when the page reached is the first: with no type before the start key
///   the page read forward from it, else an empty page whose next key is the
///   start key, both with that message, which replaces the cursor's text
///   (E-73); a failure past the page's last row, at the look-behind row the
///   program never fetches, answers the page as read;
/// - backward without a start key, the F7 on the first page from the start
///   of the table.
///
/// # Errors
///
/// - [`ErrorKind::NotFound`] with
///   [`NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS`] when no type passes the
///   valid filters (V-COTRTLIC-002), and [`ErrorKind::Unexpected`] with the
///   same text when the count fails, as the failed count leaves the record
///   count at 0 (V-COTRTLIC-028, E-73); the fields are the filters the
///   program turns not OK, the type filter first.
/// - [`ErrorKind::Invalid`] with the type code text on `TRTYPE`
///   (V-COTRTLIC-001).
/// - [`ErrorKind::Unexpected`] on `TRTYPE` with the Db2 text of the cursor
///   step that failed: for a forward request its `OPEN`
///   ([`C_TR_TYPE_FORWARD_OPEN`], V-COTRTLIC-024), the `FETCH` of a row of
///   the page ([`C_TR_TYPE_FORWARD_CLOSE`], V-COTRTLIC-007, E-73), the
///   look-ahead `FETCH` ([`C_TR_TYPE_FORWARD_FETCH`], V-COTRTLIC-006) or its
///   `CLOSE` ([`C_TR_TYPE_FORWARD_CLOSE`], V-COTRTLIC-025); for a backward
///   read whose page reached is not the first, or cannot be shown to be, its
///   `OPEN` ([`C_TR_TYPE_BACKWARD_OPEN`], V-COTRTLIC-026), a `FETCH`
///   ([`ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD`], V-COTRTLIC-010) or its
///   `CLOSE` ([`C_TR_TYPE_BACKWARD_CLOSE`], V-COTRTLIC-027). Each is
///   [`UNEXPECTED_ERROR`] for a failure without a Db2 code. A backward read
///   failure that reaches the first page gives way to
///   [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`] (V-COTRTLIC-009).
pub async fn list<S: TranTypeMasterStore>(
    store: &mut S,
    input: TranTypeListInput,
) -> Result<TranTypePage, DomainError> {
    let mut msg = ReturnMsg::default();
    let filter = edit_filters(
        input.type_filter.as_deref(),
        input.desc_filter.as_deref(),
        &mut msg,
    );
    cross_edits(store, &filter, !msg.is_off()).await?;
    if let Some((message, field)) = msg.into_parts() {
        return Err(DomainError::invalid(message).with_field(field));
    }
    let page_size = input.page_size.max(1);
    match (input.direction, input.start_key) {
        (Direction::Forward, start_key) => read_forward(store, filter, start_key, page_size).await,
        (Direction::Backward, Some(first_shown)) => {
            read_backwards(store, filter, first_shown, page_size).await
        }
        // No start key is the state the program enters with, CA-FIRST-PAGE.
        (Direction::Backward, None) => Ok(reread_first_page(store, filter, None, page_size).await),
    }
}

/// COTRTLIC 1290-CROSS-EDITS, 9100-CHECK-FILTERS — P-COTRTLIC-022,
/// P-COTRTLIC-036, P-COTRTLIC-045, V-COTRTLIC-002, V-COTRTLIC-028
///
/// `type_rejected` is `FLG-TYPEFILTER-NOT-OK`: a rejected type code puts no
/// condition in the count, but its field is still marked when the count is 0.
async fn cross_edits<S: TranTypeMasterStore>(
    store: &mut S,
    filter: &TranTypeFilter,
    type_rejected: bool,
) -> Result<(), DomainError> {
    if filter.type_code.is_none() && filter.description.is_none() {
        return Ok(());
    }
    let kind = match store.count_tran_types(filter.clone()).await {
        Ok(0) => ErrorKind::NotFound,
        Ok(_) => return Ok(()),
        // A failed COUNT assigns no count, so WS-RECORDS-COUNT keeps its 0 and
        // the no-records text replaces the Db2 text (E-73).
        Err(_) => ErrorKind::Unexpected,
    };
    let mut error = DomainError::new(kind, NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS);
    if filter.type_code.is_some() || type_rejected {
        error = error.with_field(FIELD_TYPE_FILTER);
    }
    if filter.description.is_some() {
        error = error.with_field(FIELD_DESC_FILTER);
    }
    Err(error)
}

/// One cursor read of up to `page_size` rows and the look-ahead row.
async fn browse<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    start_key: Option<TranTypeCode>,
    direction: Direction,
    page_size: u16,
) -> CursorResult<TranTypeMaster> {
    let browse = Browse {
        start_key,
        direction,
        limit: page_size.saturating_add(1),
    };
    store.browse_tran_types(browse, filter).await
}

/// The page of a forward read: the first `page_size` rows, and the code of
/// the look-ahead row as the next key (P-COTRTLIC-031).
fn forward_page(mut rows: Vec<TranTypeMaster>, page_size: u16) -> TranTypePage {
    let shown = usize::from(page_size);
    let next_key = rows.get(shown).map(|row| row.tr_type);
    rows.truncate(shown);
    TranTypePage {
        items: rows,
        next_key,
        ..TranTypePage::default()
    }
}

/// COTRTLIC 8000-READ-FORWARD, 9400-OPEN-FORWARD-CURSOR, 2500-SETUP-MESSAGE —
/// P-COTRTLIC-028, P-COTRTLIC-030, P-COTRTLIC-031, P-COTRTLIC-037,
/// V-COTRTLIC-004, V-COTRTLIC-005, V-COTRTLIC-008, V-COTRTLIC-024
///
/// A request with a start key is F8, so the previous page exists and starts
/// before the first code shown. A failed read answers the text of the cursor
/// step that failed ([`forward_cursor_error`]).
async fn read_forward<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    start_key: Option<TranTypeCode>,
    page_size: u16,
) -> Result<TranTypePage, DomainError> {
    let rows = browse(store, filter, start_key, Direction::Forward, page_size)
        .await
        .map_err(|failure| forward_cursor_error(&failure, page_size))?;
    let mut page = forward_page(rows, page_size);
    let first_shown = page.items.first().map(|row| row.tr_type);
    page.prev_key = start_key.and(first_shown);
    page.message = match (start_key, first_shown, page.next_key) {
        (None, None, _) => Some(NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC),
        // F8 past the last page shown: 2500-SETUP-MESSAGE (V-COTRTLIC-008).
        (Some(_), None, _) => Some(NO_MORE_PAGES_TO_DISPLAY_COTRTLIC),
        // F8 and the cursor answers +100 within the page or at the look-ahead.
        (Some(_), Some(_), None) => Some(NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS),
        (None, Some(_), _) | (Some(_), Some(_), Some(_)) => None,
    };
    Ok(page)
}

/// COTRTLIC 0000-MAIN F7 with CA-FIRST-PAGE, 8000-READ-FORWARD,
/// 9450-CLOSE-FORWARD-CURSOR, 2500-SETUP-MESSAGE — P-COTRTLIC-004,
/// P-COTRTLIC-028, P-COTRTLIC-038, V-COTRTLIC-009, V-COTRTLIC-025
///
/// The page is read forward from `start_key`, the first code shown, and
/// [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`] is moved over whatever the read
/// set, because `WS-DB2-ERROR` is not tested after it (E-73). A read whose
/// `CLOSE` fails has fetched the page and its look-ahead row, which stay
/// shown as after a whole read (P-COTRTLIC-038); any other failed read
/// leaves the page empty.
async fn reread_first_page<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    start_key: Option<TranTypeCode>,
    page_size: u16,
) -> TranTypePage {
    let read = browse(store, filter, start_key, Direction::Forward, page_size).await;
    let mut page = match read {
        Ok(rows) => forward_page(rows, page_size),
        Err(failure) if failure.phase == CursorPhase::Close => {
            forward_page(failure.fetched, page_size)
        }
        Err(_) => TranTypePage::default(),
    };
    page.message = Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
    page
}

/// COTRTLIC 8100-READ-BACKWARDS, 9500-OPEN-BACKWARD-CURSOR,
/// 9550-CLOSE-BACK-CURSOR, 2500-SETUP-MESSAGE — P-COTRTLIC-028,
/// P-COTRTLIC-033, P-COTRTLIC-034, P-COTRTLIC-039, P-COTRTLIC-040,
/// P-COTRTLIC-044, V-COTRTLIC-009, V-COTRTLIC-010, V-COTRTLIC-026,
/// V-COTRTLIC-027
///
/// The cursor reads the types below `first_shown` in descending order and
/// the page shows them ascending; F8 from it starts at `first_shown` again.
///
/// The COBOL counts pages and replaces the backward texts with
/// [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`] when the count reaches 1. The
/// request carries no page count (E-54), so the page reached is the first
/// when no type that passes the filters precedes it: one look-behind row
/// beyond the page tells. Fewer than a page of earlier types, the cursor's
/// +100 that E-73 reports as a fetch error from the third page on, therefore
/// always reaches the first page, the outcome of a page-back from the second
/// page.
///
/// A failed read tells nothing of the types before `first_shown`, so
/// [`failed_backward_read`] counts them before the failure is answered: on
/// a page-back that reaches the first page the first-page text replaces the
/// cursor's text, as `2500-SETUP-MESSAGE` replaces it (E-73), and on any
/// other the text of the cursor step that failed answers.
async fn read_backwards<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    first_shown: TranTypeCode,
    page_size: u16,
) -> Result<TranTypePage, DomainError> {
    let backward = browse(
        store,
        filter.clone(),
        Some(first_shown),
        Direction::Backward,
        page_size,
    )
    .await;
    let rows = match backward {
        Ok(rows) => rows,
        Err(failure) => {
            return failed_backward_read(store, filter, first_shown, page_size, failure).await;
        }
    };
    if rows.is_empty() {
        return Ok(reread_first_page(store, filter, Some(first_shown), page_size).await);
    }
    let reached_first = rows.len() <= usize::from(page_size);
    Ok(backward_page(rows, first_shown, page_size, reached_first))
}

/// The page of a backward read from `first_shown`: the first `page_size` of
/// `rows`, in the descending order the cursor fetched them, shown ascending,
/// with F8 starting at `first_shown` again. The page that `reached_first`
/// names the first has no previous key and the first-page text
/// (V-COTRTLIC-009); any other page goes back from its first code.
fn backward_page(
    mut rows: Vec<TranTypeMaster>,
    first_shown: TranTypeCode,
    page_size: u16,
    reached_first: bool,
) -> TranTypePage {
    rows.truncate(usize::from(page_size));
    rows.reverse();
    let prev_key = if reached_first {
        None
    } else {
        rows.first().map(|row| row.tr_type)
    };
    TranTypePage {
        items: rows,
        next_key: Some(first_shown),
        prev_key,
        message: reached_first.then_some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC),
    }
}

/// COTRTLIC 0000-MAIN F7, 8100-READ-BACKWARDS, 8100-READ-BACKWARDS-EXIT,
/// 2500-SETUP-MESSAGE — P-COTRTLIC-033, P-COTRTLIC-034, P-COTRTLIC-035,
/// P-COTRTLIC-039, P-COTRTLIC-040, P-COTRTLIC-044, V-COTRTLIC-009,
/// V-COTRTLIC-010, V-COTRTLIC-026, V-COTRTLIC-027
///
/// The outcome of a backward read from `first_shown` that failed with
/// `failure`. The read asks for the page and one look-behind row beyond it
/// ([`read_backwards`]); the program's loop ends at the page's last row and
/// never fetches that one, so a failure of its `FETCH` leaves the program's
/// read whole: the page is answered as read, and it is the first when the
/// count below finds no more than a page of types before `first_shown`.
///
/// Any other failure is the program's. F7 subtracts 1 from the page number
/// before `8100-READ-BACKWARDS`, and `2500-SETUP-MESSAGE` moves
/// [`NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC`] over the cursor's text whenever
/// the page reached is the first (E-73). The request carries no page number
/// (E-54), so the types before `first_shown` are counted
/// ([`types_before`]):
///
/// - none precede it: the page shown is the first, and the program's F7 on
///   the first page applies, which re-reads the page forward from
///   `first_shown` ([`reread_first_page`]);
/// - a page or fewer precede it: the page reached is the first, and the
///   failed read leaves it empty (`8100-READ-BACKWARDS` cleared the rows);
///   F8 from it starts at `first_shown` again, as after any backward read;
/// - more than a page precede it, the failed `CLOSE` having fetched the
///   look-behind row too, or the count fails: the page reached is not the
///   first, or cannot be shown to be, and the text of the cursor step that
///   failed answers ([`backward_cursor_error`]).
async fn failed_backward_read<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    first_shown: TranTypeCode,
    page_size: u16,
    failure: CursorError<TranTypeMaster>,
) -> Result<TranTypePage, DomainError> {
    let shown = usize::from(page_size);
    let preceding = match failure.phase {
        CursorPhase::Open | CursorPhase::Fetch => {
            types_before(store, filter.clone(), first_shown, page_size).await
        }
        CursorPhase::Close => None,
    };
    if failure.phase == CursorPhase::Fetch && failure.fetched.len() == shown {
        let reached_first = preceding.is_some_and(|count| count <= shown);
        return Ok(backward_page(
            failure.fetched,
            first_shown,
            page_size,
            reached_first,
        ));
    }
    match preceding {
        Some(0) => Ok(reread_first_page(store, filter, Some(first_shown), page_size).await),
        Some(count) if count <= shown => Ok(TranTypePage {
            items: Vec::new(),
            next_key: Some(first_shown),
            prev_key: None,
            message: Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC),
        }),
        _ => Err(backward_cursor_error(&failure)),
    }
}

/// The types that pass `filter` before `first_shown`, counted up to one more
/// than `page_size` by a forward read from the start of the table; `None`
/// when that read fails.
async fn types_before<S: TranTypeMasterStore>(
    store: &mut S,
    filter: TranTypeFilter,
    first_shown: TranTypeCode,
    page_size: u16,
) -> Option<usize> {
    let from_start = browse(store, filter, None, Direction::Forward, page_size)
        .await
        .ok()?;
    Some(
        from_start
            .iter()
            .take_while(|row| row.tr_type < first_shown)
            .count(),
    )
}

/// Updates the description of the listed type `code` to `new_desc`, the row
/// description entered after the `U` selector and saved with F10.
///
/// COTRTLIC 1100-RECEIVE-SCREEN, 1211-EDIT-ARRAY-DESC,
/// 1240-EDIT-ALPHANUM-REQD, 9200-UPDATE-RECORD — P-COTRTLIC-009,
/// P-COTRTLIC-010, P-COTRTLIC-013, P-COTRTLIC-017, P-COTRTLIC-021,
/// P-COTRTLIC-045, V-COTRTLIC-012, V-COTRTLIC-013, V-COTRTLIC-014,
/// V-COTRTLIC-016, V-COTRTLIC-017, V-COTRTLIC-018
///
/// The entered text is received into `X(50)`; `*` or spaces stand for no
/// description. It is compared with the listed description, both without
/// leading and trailing spaces and upper-cased: equal texts of equal length
/// skip the edits and set
/// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES`], yet the save still
/// stores the entered text, so a change of case alone is written (E-73).
/// Otherwise the description must be given and hold only letters, digits and
/// spaces. The stored description is the entered text without its leading
/// and trailing spaces; the master column keeps no trailing spaces, so the
/// 50-byte length the COBOL writes is not kept.
///
/// The COBOL compares with the description its COMMAREA holds from the list
/// shown. The stateless request has none, so the listed description is the
/// one stored now; a type no longer stored compares as changed and its
/// update answers the not-found outcome.
///
/// # Errors
///
/// - [`ErrorKind::Empty`] with the [`FIELD_TRANSACTION_DESC`]
///   [`MUST_BE_SUPPLIED`] text (V-COTRTLIC-013) and [`ErrorKind::Invalid`]
///   with the [`CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY`] one (V-COTRTLIC-014).
/// - When the update fails: [`ErrorKind::NotFound`] with the
///   [`RECORD_NOT_FOUND_DELETED_BY_OTHERS`] Db2 text when no row has the code
///   (V-COTRTLIC-016); [`ErrorKind::Unexpected`] with the
///   [`DEADLOCK_SOMEONE_ELSE_UPDATING`] text on a lock failure
///   (V-COTRTLIC-017) and the [`UPDATE_FAILED_WITH`] text on any other Db2
///   code (V-COTRTLIC-018), or [`UNEXPECTED_ERROR`] for a failure without
///   one. After the no-change text the failure keeps that text and only its
///   kind applies (V-COTRTLIC-012).
/// - A failed read of the listed description answers as the update would
///   have failed.
pub async fn update_description<S: TranTypeMasterStore>(
    store: &mut S,
    code: TranTypeCode,
    new_desc: &str,
) -> Result<TranTypeUpdated, DomainError> {
    let entered = receive_row_description(new_desc);
    let listed = match store.read_tran_type(code).await {
        Ok(master) => Some(master.tr_description),
        Err(PortError::NotFound) => None,
        Err(error) => return Err(update_failure(&error, false)),
    };
    let unchanged = match (&entered, &listed) {
        (Some(text), Some(listed)) => is_unchanged(text, listed),
        _ => false,
    };
    let description = match entered {
        Some(text) if unchanged => text,
        entered => edit_alphanum_reqd(entered)?,
    };
    // The edits, or the match with a stored description, leave at most 50
    // ISO 8859-1 characters, which the column holds; the check stays so that
    // no unchecked text reaches the store.
    let master = TranTypeMaster::try_new(code, description)
        .map_err(|_| DomainError::unexpected(UNEXPECTED_ERROR))?;
    store
        .update_tran_type(master.clone())
        .await
        .map_err(|error| update_failure(&error, unchanged))?;
    let message = if unchanged {
        NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES
    } else {
        HIGHLIGHTED_ROW_WAS_UPDATED
    };
    Ok(TranTypeUpdated {
        transaction_type: master,
        message,
        info: HIGHLIGHTED_ROW_WAS_UPDATED,
    })
}

/// COTRTLIC 1100-RECEIVE-SCREEN — P-COTRTLIC-013
///
/// `TRTYPDI X(50)` holding `*` or spaces leaves `WS-ROW-TR-DESC-IN` at
/// `LOW-VALUES`, given here as `None`; any other text is moved in
/// ([`receive_description`]) without its leading and trailing spaces.
fn receive_row_description(entered: &str) -> Option<String> {
    let field = receive_description(entered);
    let text = field.trim_end_matches(' ');
    if text.is_empty() || text == LIT_ASTERISK {
        return None;
    }
    Some(text.trim_start_matches(' ').to_owned())
}

/// COTRTLIC 1211-EDIT-ARRAY-DESC — P-COTRTLIC-017, V-COTRTLIC-012
///
/// `entered` is already trimmed. The texts are compared as `FUNCTION
/// UPPER-CASE` leaves them, which folds the letters `a` to `z` only.
fn is_unchanged(entered: &str, listed: &str) -> bool {
    let listed = listed.trim_matches(' ');
    entered.chars().count() == listed.chars().count() && entered.eq_ignore_ascii_case(listed)
}

/// COTRTLIC 1240-EDIT-ALPHANUM-REQD with [`FIELD_TRANSACTION_DESC`], length
/// 50 — P-COTRTLIC-021, V-COTRTLIC-013, V-COTRTLIC-014
///
/// The work field is the row description in `X(50)`
/// ([`receive_description`]): `LOW-VALUES` (`None`), or a text that fills it
/// with spaces or low-values, is not supplied. `INSPECT … CONVERTING` turns
/// `A-Z`, `a-z` and `0-9` to spaces, and any character it leaves other than
/// a space rejects the text, a character outside ASCII included.
fn edit_alphanum_reqd(entered: Option<String>) -> Result<String, DomainError> {
    let supplied = entered.filter(|text| !is_spaces_or_low_values(&receive_description(text)));
    let Some(text) = supplied else {
        return Err(DomainError::empty(field_edit_message(
            FIELD_TRANSACTION_DESC,
            MUST_BE_SUPPLIED,
        )));
    };
    if text
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == ' ')
    {
        Ok(text)
    } else {
        Err(DomainError::invalid(field_edit_message(
            FIELD_TRANSACTION_DESC,
            CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
        )))
    }
}

/// COTRTLIC 9200-UPDATE-RECORD — P-COTRTLIC-010, P-COTRTLIC-045,
/// V-COTRTLIC-012, V-COTRTLIC-016, V-COTRTLIC-017, V-COTRTLIC-018
///
/// The Db2 texts are moved only while `WS-RETURN-MSG` is blank, so after the
/// no-change text (`unchanged`) that text stays. A missing row is
/// [`ErrorKind::NotFound`] and every other failure [`ErrorKind::Unexpected`]
/// (E-50).
fn update_failure(error: &PortError, unchanged: bool) -> DomainError {
    let (kind, action) = match error {
        PortError::NotFound => (ErrorKind::NotFound, RECORD_NOT_FOUND_DELETED_BY_OTHERS),
        PortError::Contention => (ErrorKind::Unexpected, DEADLOCK_SOMEONE_ELSE_UPDATING),
        PortError::Duplicate | PortError::Restricted | PortError::Unavailable(_) => {
            (ErrorKind::Unexpected, UPDATE_FAILED_WITH)
        }
    };
    if unchanged {
        return DomainError::new(kind, NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES);
    }
    db2_error(kind, action, error)
}

/// Deletes the listed type `code`, the row selected with `D` and confirmed
/// with F10.
///
/// COTRTLIC 9300-DELETE-RECORD — P-COTRTLIC-007, P-COTRTLIC-008,
/// V-COTRTLIC-021, V-COTRTLIC-022, V-COTRTLIC-023
///
/// The success text is the `COTRTLIC` one; the `COTRTUPC` delete and its
/// texts belong to `super::tran_type_update` (E-23).
///
/// # Errors
///
/// - [`ErrorKind::Conflict`] with the
///   [`PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST`] Db2 text when
///   categories still reference the type (SQLCODE -532, V-COTRTLIC-021).
/// - [`ErrorKind::NotFound`] with the [`DELETE_FAILED_WITH_MESSAGE`] Db2 text
///   when no row has the code (SQLCODE +100, V-COTRTLIC-022).
/// - [`ErrorKind::Unexpected`] with the [`DELETE_FAILED_WITH_MESSAGE`] text on
///   any other Db2 code (V-COTRTLIC-023), or [`UNEXPECTED_ERROR`] for a
///   failure without one.
pub async fn delete<S: TranTypeMasterStore>(
    store: &mut S,
    code: TranTypeCode,
) -> Result<TranTypeDeleted, DomainError> {
    store
        .delete_tran_type(code)
        .await
        .map_err(|error| delete_failure(&error))?;
    Ok(TranTypeDeleted {
        type_code: code,
        message: HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
    })
}

/// COTRTLIC 9300-DELETE-RECORD — P-COTRTLIC-008, V-COTRTLIC-021,
/// V-COTRTLIC-022, V-COTRTLIC-023
///
/// The child-record conflict is [`ErrorKind::Conflict`], a missing row
/// [`ErrorKind::NotFound`] and every other failure
/// [`ErrorKind::Unexpected`] (E-50).
fn delete_failure(error: &PortError) -> DomainError {
    let (kind, action) = match error {
        PortError::Restricted => (
            ErrorKind::Conflict,
            PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST,
        ),
        PortError::NotFound => (ErrorKind::NotFound, DELETE_FAILED_WITH_MESSAGE),
        PortError::Duplicate | PortError::Contention | PortError::Unavailable(_) => {
            (ErrorKind::Unexpected, DELETE_FAILED_WITH_MESSAGE)
        }
    };
    db2_error(kind, action, error)
}

/// COTRTLIC 8000-READ-FORWARD, 9400-OPEN-FORWARD-CURSOR,
/// 9450-CLOSE-FORWARD-CURSOR — P-COTRTLIC-031, P-COTRTLIC-032,
/// P-COTRTLIC-037, P-COTRTLIC-038, P-COTRTLIC-044, V-COTRTLIC-006,
/// V-COTRTLIC-007, V-COTRTLIC-024, V-COTRTLIC-025
///
/// The error of a forward read of `page_size` rows and the look-ahead row
/// that failed with `failure`, by the cursor step that failed:
///
/// - the `OPEN`: [`C_TR_TYPE_FORWARD_OPEN`] (V-COTRTLIC-024);
/// - the `FETCH` of a row of the page: [`C_TR_TYPE_FORWARD_CLOSE`], the
///   action text the loop moves although the `FETCH` failed (V-COTRTLIC-007,
///   E-73);
/// - the look-ahead `FETCH` after a full page: [`C_TR_TYPE_FORWARD_FETCH`]
///   (V-COTRTLIC-006);
/// - the `CLOSE` after the look-ahead row: [`C_TR_TYPE_FORWARD_CLOSE`]
///   (V-COTRTLIC-025).
///
/// The texts are moved only while `WS-RETURN-MSG` is blank, and no list
/// request reaches the read with a text set (V-COTRTLIC-024), so the first
/// step that fails gives the text: the `CLOSE` after a failed `FETCH` keeps
/// the `FETCH`'s.
fn forward_cursor_error(failure: &CursorError<TranTypeMaster>, page_size: u16) -> DomainError {
    let action = match failure.phase {
        CursorPhase::Open => C_TR_TYPE_FORWARD_OPEN,
        CursorPhase::Fetch if failure.fetched.len() >= usize::from(page_size) => {
            C_TR_TYPE_FORWARD_FETCH
        }
        CursorPhase::Fetch | CursorPhase::Close => C_TR_TYPE_FORWARD_CLOSE,
    };
    cursor_error(action, &failure.error)
}

/// COTRTLIC 8100-READ-BACKWARDS, 8100-READ-BACKWARDS-EXIT,
/// 9500-OPEN-BACKWARD-CURSOR, 9550-CLOSE-BACK-CURSOR — P-COTRTLIC-034,
/// P-COTRTLIC-035, P-COTRTLIC-039, P-COTRTLIC-040, P-COTRTLIC-044,
/// V-COTRTLIC-010, V-COTRTLIC-026, V-COTRTLIC-027
///
/// The error of a failed backward read on a page that is not the first, by
/// the cursor step that failed: the `OPEN`, [`C_TR_TYPE_BACKWARD_OPEN`],
/// which the `FETCH` and the `CLOSE` of the unopened cursor keep
/// (V-COTRTLIC-026); a `FETCH`, [`ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD`]
/// (V-COTRTLIC-010); the `CLOSE`, [`C_TR_TYPE_BACKWARD_CLOSE`]
/// (V-COTRTLIC-027). As forward, the first step that fails gives the text.
fn backward_cursor_error(failure: &CursorError<TranTypeMaster>) -> DomainError {
    let action = match failure.phase {
        CursorPhase::Open => C_TR_TYPE_BACKWARD_OPEN,
        CursorPhase::Fetch => ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD,
        CursorPhase::Close => C_TR_TYPE_BACKWARD_CLOSE,
    };
    cursor_error(action, &failure.error)
}

/// A failed cursor read of the list: the Db2 text of the cursor's `action`
/// for `error`, on the type filter, where the program puts the cursor when
/// no input is in error.
fn cursor_error(action: &str, error: &PortError) -> DomainError {
    db2_error(ErrorKind::Unexpected, action, error).with_field(FIELD_TYPE_FILTER)
}

/// COTRTLIC 9999-FORMAT-DB2-MESSAGE — P-COTRTLIC-043, V-COTRTLIC-016 to
/// V-COTRTLIC-018, V-COTRTLIC-021 to V-COTRTLIC-024, V-COTRTLIC-026
///
/// The message of `kind` for the Db2 `action`, carrying the SQLCODE of
/// `error`. A failure without a Db2 code is [`UNEXPECTED_ERROR`]: its detail
/// is for logs only (module docs).
fn db2_error(kind: ErrorKind, action: &str, error: &PortError) -> DomainError {
    match error.sqlcode() {
        Some(sqlcode) => DomainError::new(
            kind,
            tran_type_list_db2_message(action, sqlcode, NO_DSNTIAC_TEXT),
        ),
        None => DomainError::unexpected(UNEXPECTED_ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FIELD_DESC_FILTER, FIELD_TYPE_FILTER, SCREEN_ROWS, TranTypeDeleted, TranTypeListInput,
        TranTypePage, TranTypeUpdated, delete, edit_filters, list, update_description,
    };
    use crate::entities::{TranCategoryMaster, TranTypeMaster};
    use crate::error::DomainError;
    use crate::format::edit_sqlcode;
    use crate::messages::{
        C_TR_TYPE_BACKWARD_CLOSE, C_TR_TYPE_BACKWARD_OPEN, C_TR_TYPE_FORWARD_CLOSE,
        C_TR_TYPE_FORWARD_FETCH, C_TR_TYPE_FORWARD_OPEN, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
        DEADLOCK_SOMEONE_ELSE_UPDATING, DELETE_FAILED_WITH_MESSAGE,
        ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD, FIELD_TRANSACTION_DESC,
        HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE, HIGHLIGHTED_ROW_WAS_UPDATED,
        MUST_BE_SUPPLIED, NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES,
        NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS, NO_MORE_PAGES_TO_DISPLAY_COTRTLIC,
        NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC, NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS,
        NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC,
        PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST, RECORD_NOT_FOUND_DELETED_BY_OTHERS,
        SQLCODE_LABEL, TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT, UNEXPECTED_ERROR,
        UPDATE_FAILED_WITH, field_edit_message, tran_type_list_db2_message, with_sqlcode,
    };
    use crate::numeric::{Alnum, TranCategoryCode, TranTypeCode};
    use crate::ports::{Direction, PortError, TranTypeFilter};
    use crate::rules::ReturnMsg;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The Db2 load of the type master, `'REVERAL'` included
    /// [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:L17-23].
    const DB2_TYPES: [(&str, &str); 7] = [
        ("01", "PURCHASE"),
        ("02", "PAYMENT"),
        ("03", "CREDIT"),
        ("04", "AUTHORIZATION"),
        ("05", "REFUND"),
        ("06", "REVERAL"),
        ("07", "ADJUSTMENT"),
    ];

    const FORWARD: Direction = Direction::Forward;
    const BACKWARD: Direction = Direction::Backward;

    fn code(text: &str) -> TranTypeCode {
        TranTypeCode::move_from(text)
    }

    /// The key `text` names; the empty text names none.
    fn key(text: &str) -> Option<TranTypeCode> {
        (!text.is_empty()).then(|| code(text))
    }

    fn master(type_code: &str, description: &str) -> TranTypeMaster {
        TranTypeMaster {
            tr_type: code(type_code),
            tr_description: description.to_owned(),
        }
    }

    /// The Db2 load, with the category `01`/`0001` that restricts a delete of
    /// type 01 [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20].
    fn seeded() -> FakeStore {
        let store = DB2_TYPES
            .iter()
            .fold(FakeStore::default(), |store, (type_code, description)| {
                store.with_tran_type(master(type_code, description))
            });
        store.with_tran_category(TranCategoryMaster {
            trc_type_code: code("01"),
            trc_type_category: TranCategoryCode::new(1).expect("a four-digit category"),
            trc_cat_data: Alnum::move_from("REGULAR SALES DRAFT"),
        })
    }

    /// Types `01` to `count`, each described `TYPE <code>`.
    fn numbered(count: u8) -> FakeStore {
        (1..=count).fold(FakeStore::default(), |store, number| {
            let type_code = format!("{number:02}");
            store.with_tran_type(master(&type_code, &format!("TYPE {type_code}")))
        })
    }

    fn codes(items: &[TranTypeMaster]) -> Vec<String> {
        items.iter().map(|row| row.tr_type.to_string()).collect()
    }

    fn filters(type_filter: Option<&str>, desc_filter: Option<&str>) -> TranTypeListInput {
        TranTypeListInput {
            type_filter: type_filter.map(str::to_owned),
            desc_filter: desc_filter.map(str::to_owned),
            ..TranTypeListInput::default()
        }
    }

    fn browse(start_key: &str, direction: Direction) -> TranTypeListInput {
        TranTypeListInput {
            start_key: key(start_key),
            direction,
            ..TranTypeListInput::default()
        }
    }

    /// The 9999-FORMAT-DB2-MESSAGE text of `action` with an empty DSNTIAC
    /// slot (E-30).
    fn db2_text(action: &str, sqlcode: i32) -> String {
        tran_type_list_db2_message(action, sqlcode, "")
    }

    /// A page holding only `message`.
    fn empty_page(message: &'static str) -> TranTypePage {
        TranTypePage {
            message: Some(message),
            ..TranTypePage::default()
        }
    }

    #[test]
    fn edit_filters_classify_the_type_and_description_filters() {
        // P-COTRTLIC-018, P-COTRTLIC-020, P-COTRTLIC-003, V-COTRTLIC-001.
        let low_values = "\0".repeat(50);
        // A character the 50-byte cut splits is kept whole, and the cut still
        // drops what follows it.
        let split_two = format!("{}\u{e9}", "A".repeat(49));
        let split_four = format!("{}\u{1f600}", "A".repeat(48));
        let split_then_more = format!("{split_two}B");
        let (two_pattern, four_pattern) = (format!("%{split_two}%"), format!("%{split_four}%"));
        let cases = [
            (None, None, "", None, false),
            (Some(""), Some(""), "", None, false),
            (Some("  "), Some("   "), "", None, false),
            (Some("00"), Some(low_values.as_str()), "", None, false),
            (Some("\0\0"), None, "", None, false),
            (Some("01"), None, "01", None, false),
            (Some("0A"), None, "", None, true),
            (Some("1"), None, "", None, true),
            (Some("A1"), None, "", None, true),
            (None, Some("PAY"), "", Some("%PAY%"), false),
            (None, Some("  pay  "), "", Some("%pay%"), false),
            (None, Some("10%_"), "", Some("%10%_%"), false),
            (Some("0A"), Some("PAY"), "", Some("%PAY%"), true),
            (None, Some("\u{e9}"), "", Some("%\u{e9}%"), false),
            (None, Some(" Caf\u{e9} "), "", Some("%Caf\u{e9}%"), false),
            (
                None,
                Some(split_two.as_str()),
                "",
                Some(two_pattern.as_str()),
                false,
            ),
            (
                None,
                Some(split_four.as_str()),
                "",
                Some(four_pattern.as_str()),
                false,
            ),
            (
                None,
                Some(split_then_more.as_str()),
                "",
                Some(two_pattern.as_str()),
                false,
            ),
        ];
        for (type_filter, desc_filter, type_code, pattern, rejected) in cases {
            let mut msg = ReturnMsg::default();
            let filter = edit_filters(type_filter, desc_filter, &mut msg);
            let expected = TranTypeFilter {
                type_code: key(type_code),
                description: pattern.map(str::to_owned),
            };
            assert_eq!(filter, expected, "{type_filter:?} {desc_filter:?}");
            let text = rejected.then_some(TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT);
            assert_eq!(msg.message(), text, "{type_filter:?}");
            assert_eq!(msg.field(), rejected.then_some(FIELD_TYPE_FILTER));
        }
    }

    #[test]
    fn the_first_page_lists_seven_types_in_code_order_with_the_look_ahead() {
        // P-COTRTLIC-003, P-COTRTLIC-030, P-COTRTLIC-031: an unfiltered list
        // counts nothing and keeps the eighth code for F8.
        assert_eq!(SCREEN_ROWS, 7);
        let mut store = numbered(9);
        let page = block_on(list(&mut store, TranTypeListInput::default())).expect("a page");
        let first_seven = ["01", "02", "03", "04", "05", "06", "07"];
        assert_eq!(codes(&page.items), first_seven);
        assert_eq!(page.items[0], master("01", "TYPE 01"));
        assert_eq!((page.next_key, page.prev_key), (key("08"), None));
        assert_eq!(page.message, None);
        assert_eq!(store.calls, ["browse_tran_types"]);

        let mut store = seeded();
        let page = block_on(list(&mut store, TranTypeListInput::default())).expect("a page");
        assert_eq!(codes(&page.items), first_seven);
        assert_eq!((page.next_key, page.message), (None, None));

        // The page size the request resolves; 0 counts as 1.
        for (page_size, shown, next) in [(3, 3, "04"), (0, 1, "02")] {
            let input = TranTypeListInput {
                page_size,
                ..TranTypeListInput::default()
            };
            let page = block_on(list(&mut numbered(9), input)).expect("a page");
            assert_eq!(page.items.len(), shown, "{page_size}");
            assert_eq!(page.next_key, key(next), "{page_size}");
        }
    }

    #[test]
    fn an_empty_master_lists_no_records_for_this_search_condition() {
        // V-COTRTLIC-004, P-COTRTLIC-030.
        let mut store = FakeStore::default();
        let page = block_on(list(&mut store, TranTypeListInput::default()))
            .expect("an empty page is not an error");
        let expected = empty_page(NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COTRTLIC);
        assert_eq!(page, expected);
    }

    #[test]
    fn browsing_pages_forward_and_backward_by_the_codes_shown() {
        // V-COTRTLIC-005, V-COTRTLIC-008, V-COTRTLIC-009, P-COTRTLIC-004,
        // P-COTRTLIC-031, P-COTRTLIC-033, P-COTRTLIC-034, P-COTRTLIC-044.
        let more = Some(NO_MORE_PAGES_FOR_THESE_SEARCH_CONDITIONS);
        let first = Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
        let cases = [
            (16, "08", FORWARD, (8, 14), "15", "08", None),
            (9, "08", FORWARD, (8, 9), "", "08", more),
            (7, "07", FORWARD, (7, 7), "", "07", more),
            (16, "15", BACKWARD, (8, 14), "15", "08", None),
            (16, "08", BACKWARD, (1, 7), "08", "", first),
            (9, "03", BACKWARD, (1, 2), "03", "", first),
            (9, "01", BACKWARD, (1, 7), "08", "", first),
            (9, "", BACKWARD, (1, 7), "08", "", first),
        ];
        for (count, start, direction, (low, high), next, prev, message) in cases {
            let mut store = numbered(count);
            let input = browse(start, direction);
            let page = block_on(list(&mut store, input)).expect("a page");
            let expected: Vec<String> = (low..=high).map(|n| format!("{n:02}")).collect();
            let case = format!("{count} {start:?} {direction:?}");
            assert_eq!(codes(&page.items), expected, "{case}");
            assert_eq!(page.next_key, key(next), "{case}");
            assert_eq!(page.prev_key, key(prev), "{case}");
            assert_eq!(page.message, message, "{case}");
        }

        // Forward past the last code: nothing is left to show.
        let page = block_on(list(&mut numbered(9), browse("10", FORWARD))).expect("a page");
        assert_eq!(page, empty_page(NO_MORE_PAGES_TO_DISPLAY_COTRTLIC));
    }

    #[test]
    fn filters_select_the_types_the_cursor_conditions_match() {
        // P-COTRTLIC-003, P-COTRTLIC-020: TR_TYPE = code and a case-sensitive
        // LIKE whose '%' and '_' are wildcards.
        let cases = [
            (Some("03"), None, vec!["03"]),
            (None, Some("PAY"), vec!["02"]),
            (None, Some(" RE "), vec!["03", "05", "06"]),
            (None, Some("_A"), vec!["01", "02", "04", "06"]),
            (Some("01"), Some("PUR"), vec!["01"]),
            (Some("00"), Some("ADJ"), vec!["07"]),
        ];
        for (type_filter, desc_filter, expected) in cases {
            let mut store = seeded();
            let input = filters(type_filter, desc_filter);
            let page = block_on(list(&mut store, input)).expect("a filtered page");
            let case = format!("{type_filter:?} {desc_filter:?}");
            assert_eq!(codes(&page.items), expected, "{case}");
            assert_eq!(page.next_key, None, "{case}");
            assert_eq!((page.prev_key, page.message), (None, None), "{case}");
            assert_eq!(store.calls, ["count_tran_types", "browse_tran_types"]);
        }
    }

    #[test]
    fn filters_that_fail_their_edits_or_match_nothing_are_rejected() {
        // V-COTRTLIC-001, V-COTRTLIC-002, P-COTRTLIC-014, P-COTRTLIC-022.
        let type_code = DomainError::invalid(TYPE_CODE_FILTER_IF_SUPPLIED_MUST_BE_2_DIGIT)
            .with_field(FIELD_TYPE_FILTER);
        let none = DomainError::not_found(NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS);
        let on_type = none.clone().with_field(FIELD_TYPE_FILTER);
        let on_desc = none.with_field(FIELD_DESC_FILTER);
        let on_both = on_type.clone().with_field(FIELD_DESC_FILTER);
        // No seeded description holds a character outside ASCII, so a filter
        // that keeps one matches nothing.
        let split = format!("{}\u{e9}", "A".repeat(49));
        let cases = [
            (Some("0A"), None, &type_code),
            (Some("1"), None, &type_code),
            (Some("0A"), Some("PAY"), &type_code),
            (Some("99"), None, &on_type),
            (None, Some("ZZZ"), &on_desc),
            (None, Some("pay"), &on_desc),
            (None, Some("\u{e9}"), &on_desc),
            (None, Some("PURCHAS\u{e9}"), &on_desc),
            (None, Some(split.as_str()), &on_desc),
            (Some("01"), Some("PAY"), &on_both),
            (Some("0A"), Some("ZZZ"), &on_both),
        ];
        for (type_filter, desc_filter, expected) in cases {
            let mut store = seeded();
            let outcome = block_on(list(&mut store, filters(type_filter, desc_filter)));
            let case = format!("{type_filter:?} {desc_filter:?}");
            assert_eq!(outcome.as_ref(), Err(expected), "{case}");
            assert!(!store.calls.contains(&"browse_tran_types"), "{case}");
        }
    }

    #[test]
    fn a_failed_filter_count_reads_as_a_filter_matching_nothing() {
        // V-COTRTLIC-028, P-COTRTLIC-036, P-COTRTLIC-045 (E-73).
        let expected = DomainError::unexpected(NO_RECORDS_FOUND_FOR_THESE_FILTER_CONDITIONS)
            .with_field(FIELD_DESC_FILTER);
        let unavailable = PortError::Unavailable("down".to_owned());
        for error in [PortError::Contention, unavailable] {
            let mut store = seeded();
            store.fail_next("count_tran_types", error.clone());
            let outcome = block_on(list(&mut store, filters(None, Some("PAY"))));
            assert_eq!(outcome, Err(expected.clone()), "{error:?}");
        }
    }

    #[test]
    fn db2_texts_carry_the_action_the_edited_sqlcode_and_an_empty_dsntiac_slot() {
        // P-COTRTLIC-043 (E-30): the composed text the error rows expect.
        let label = |action: &str| [action.trim_end_matches(' '), SQLCODE_LABEL].concat();
        let cases = [
            (C_TR_TYPE_FORWARD_OPEN, -911),
            (RECORD_NOT_FOUND_DELETED_BY_OTHERS, 100),
            (PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST, -532),
        ];
        for (action, sqlcode) in cases {
            let expected = [with_sqlcode(&label(action), sqlcode).as_str(), " "].concat();
            assert_eq!(db2_text(action, sqlcode), expected);
            assert!(expected.contains(&edit_sqlcode(sqlcode)), "{expected:?}");
        }
    }

    #[test]
    fn cursor_failures_answer_the_open_texts_unless_the_first_page_is_reread() {
        // V-COTRTLIC-024, V-COTRTLIC-026, V-COTRTLIC-009, P-COTRTLIC-037,
        // P-COTRTLIC-039.
        let forward = C_TR_TYPE_FORWARD_OPEN;
        let backward = C_TR_TYPE_BACKWARD_OPEN;
        // Backward from 15 of 16 types: fourteen precede it, so the page
        // reached is not the first.
        let cases = [
            (9, "", FORWARD, PortError::Contention, forward, -911),
            (9, "08", FORWARD, PortError::Duplicate, forward, -803),
            (16, "15", BACKWARD, PortError::Contention, backward, -911),
        ];
        for (count, start, direction, error, action, sqlcode) in cases {
            let mut store = numbered(count);
            store.fail_next("browse_tran_types", error);
            let text = db2_text(action, sqlcode);
            let expected = DomainError::unexpected(text).with_field(FIELD_TYPE_FILTER);
            let outcome = block_on(list(&mut store, browse(start, direction)));
            assert_eq!(outcome, Err(expected), "{count} {start:?} {direction:?}");
        }

        // The backward read and the count of the types before it both fail:
        // the first page cannot be shown to be reached, and the backward
        // read's own failure answers.
        let mut store = numbered(9);
        store.fail_next("browse_tran_types", PortError::Contention);
        store.fail_next("browse_tran_types", PortError::Duplicate);
        let outcome = block_on(list(&mut store, browse("08", BACKWARD)));
        let text = db2_text(backward, -911);
        let expected = DomainError::unexpected(text).with_field(FIELD_TYPE_FILTER);
        assert_eq!(outcome, Err(expected));
        assert_eq!(store.calls, ["browse_tran_types", "browse_tran_types"]);

        let unavailable = PortError::Unavailable("connection reset".to_owned());
        let mut store = numbered(9);
        store.fail_next("browse_tran_types", unavailable.clone());
        let outcome = block_on(list(&mut store, TranTypeListInput::default()));
        let expected = DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_TYPE_FILTER);
        assert_eq!(outcome, Err(expected));

        // F7 on the first page: the re-read's failure gives way to the
        // first-page text, and the page is empty.
        for error in [PortError::Contention, unavailable] {
            let mut store = numbered(9);
            store.fail_next("browse_tran_types", error.clone());
            let page = block_on(list(&mut store, browse("", BACKWARD)))
                .expect("the first-page text is not an error");
            let expected = empty_page(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
            assert_eq!(page, expected, "{error:?}");
        }
    }

    #[test]
    fn a_failed_backward_read_that_reaches_the_first_page_answers_the_first_page_text() {
        // V-COTRTLIC-009, P-COTRTLIC-039, P-COTRTLIC-044 (E-73): F7 from the
        // second page reaches the first, whose text replaces the cursor's; the
        // failed read leaves the page empty, F8 starting at the code that was
        // first shown. From the first page, F7 re-reads it forward.
        let type_1 = TranTypeListInput {
            start_key: key("15"),
            direction: BACKWARD,
            ..filters(None, Some("TYPE 1"))
        };
        let first = Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
        let first_seven = vec!["01", "02", "03", "04", "05", "06", "07"];
        let cases = [
            (9, browse("08", BACKWARD), vec![], "08"),
            (9, browse("03", BACKWARD), vec![], "03"),
            (9, browse("01", BACKWARD), first_seven, "08"),
            (16, type_1, vec![], "15"),
        ];
        for (count, input, shown, next) in cases {
            let unavailable = PortError::Unavailable("connection reset".to_owned());
            for error in [PortError::Contention, unavailable] {
                let mut store = numbered(count);
                store.fail_next("browse_tran_types", error.clone());
                let case = format!("{count} {input:?} {error:?}");
                let page = block_on(list(&mut store, input.clone()))
                    .expect("the first-page text is not an error");
                assert_eq!(codes(&page.items), shown, "{case}");
                assert_eq!((page.next_key, page.prev_key), (key(next), None), "{case}");
                assert_eq!(page.message, first, "{case}");
            }
        }
    }

    #[test]
    fn a_failed_forward_read_answers_the_text_of_the_cursor_step_that_failed() {
        // V-COTRTLIC-006, V-COTRTLIC-007, V-COTRTLIC-024, V-COTRTLIC-025,
        // P-COTRTLIC-031, P-COTRTLIC-032, P-COTRTLIC-037, P-COTRTLIC-038,
        // P-COTRTLIC-044 (E-73: a failed FETCH of the page names the close).
        let (open, close) = (C_TR_TYPE_FORWARD_OPEN, C_TR_TYPE_FORWARD_CLOSE);
        let fetch = C_TR_TYPE_FORWARD_FETCH;
        // The rows fetched before the failure: none is the OPEN, fewer than
        // the page's seven a FETCH of the page, seven the look-ahead FETCH,
        // and the eight rows asked for the CLOSE.
        let cases = [
            (16, "", 0, open),
            (16, "", 1, close),
            (16, "", 6, close),
            (16, "", 7, fetch),
            (16, "", 8, close),
            (16, "08", 3, close),
            (16, "08", 7, fetch),
            // The FETCH that would answer +100 after the last type fails: in
            // the page after five types, as the look-ahead after seven.
            (5, "", 5, close),
            (7, "", 7, fetch),
        ];
        for (count, start, fetched, action) in cases {
            let case = format!("{count} {start:?} {fetched}");
            let mut store = numbered(count);
            store.fail_tran_type_cursor(fetched, PortError::Contention);
            let outcome = block_on(list(&mut store, browse(start, FORWARD)));
            let expected =
                DomainError::unexpected(db2_text(action, -911)).with_field(FIELD_TYPE_FILTER);
            assert_eq!(outcome, Err(expected), "{case}");
            assert_eq!(store.calls, ["browse_tran_types"], "{case}");

            // A failure without a Db2 code answers the unexpected-error text.
            let mut store = numbered(count);
            let unavailable = PortError::Unavailable("connection reset".to_owned());
            store.fail_tran_type_cursor(fetched, unavailable);
            let outcome = block_on(list(&mut store, browse(start, FORWARD)));
            let expected = DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_TYPE_FILTER);
            assert_eq!(outcome, Err(expected), "{case}");

            // F7 on the first page re-reads it forward, and the first-page
            // text replaces whichever text the failed step gave. A failed
            // CLOSE comes after the page and its look-ahead row, which stay
            // shown (P-COTRTLIC-038); any earlier failure leaves no row.
            let mut store = numbered(count);
            store.fail_tran_type_cursor(fetched, PortError::Contention);
            let page = block_on(list(&mut store, browse("", BACKWARD)))
                .expect("the first-page text is not an error");
            let expected = if fetched > usize::from(SCREEN_ROWS) {
                let mut whole = block_on(list(&mut numbered(count), browse("", FORWARD)))
                    .expect("the first page of a whole read");
                whole.message = Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC);
                whole
            } else {
                empty_page(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC)
            };
            assert_eq!(page, expected, "{case}");
        }
    }

    #[test]
    fn a_failed_backward_read_answers_the_text_of_the_cursor_step_that_failed() {
        // V-COTRTLIC-010, V-COTRTLIC-026, V-COTRTLIC-027, P-COTRTLIC-034,
        // P-COTRTLIC-035, P-COTRTLIC-039, P-COTRTLIC-040: backward from 15 of
        // 16 types, whose fourteen predecessors put the page reached after
        // the first. The OPEN and a FETCH leave the types before 15 to be
        // counted; the CLOSE comes after the look-behind row, which tells
        // that more than a page precede.
        let fetch = ERROR_ON_FETCH_CURSOR_C_TR_TYPE_BACKWARD;
        let cases = [
            (0, C_TR_TYPE_BACKWARD_OPEN, 2),
            (1, fetch, 2),
            (6, fetch, 2),
            (8, C_TR_TYPE_BACKWARD_CLOSE, 1),
        ];
        for (fetched, action, reads) in cases {
            let mut store = numbered(16);
            store.fail_tran_type_cursor(fetched, PortError::Contention);
            let outcome = block_on(list(&mut store, browse("15", BACKWARD)));
            let expected =
                DomainError::unexpected(db2_text(action, -911)).with_field(FIELD_TYPE_FILTER);
            assert_eq!(outcome, Err(expected), "{fetched}");
            assert_eq!(store.calls, vec!["browse_tran_types"; reads], "{fetched}");
        }

        // A FETCH failure on a page-back that reaches the first page gives way
        // to the first-page text, the page empty and F8 starting at 08 again.
        let mut store = numbered(16);
        store.fail_tran_type_cursor(3, PortError::Contention);
        let page = block_on(list(&mut store, browse("08", BACKWARD))).expect("the first page");
        let expected = TranTypePage {
            next_key: key("08"),
            ..empty_page(NO_PREVIOUS_PAGES_TO_DISPLAY_COTRTLIC)
        };
        assert_eq!(page, expected);
    }

    #[test]
    fn a_failure_at_the_look_behind_row_answers_the_page_as_read() {
        // P-COTRTLIC-033: the program's backward loop ends at the page's
        // seventh row and never fetches the look-behind row, so a failure there
        // leaves its read whole. The count of the types before the start key
        // tells whether the page reached is the first.
        for start in ["15", "08"] {
            let expected = block_on(list(&mut numbered(16), browse(start, BACKWARD)))
                .expect("the page of a whole read");
            let mut store = numbered(16);
            store.fail_tran_type_cursor(SCREEN_ROWS.into(), PortError::Contention);
            let page = block_on(list(&mut store, browse(start, BACKWARD))).expect("the page");
            assert_eq!(page, expected, "{start}");
            assert_eq!(store.calls, ["browse_tran_types", "browse_tran_types"]);
        }

        // The count fails too: the page is shown, not shown to be the first.
        let mut store = numbered(16);
        store.fail_tran_type_cursor(SCREEN_ROWS.into(), PortError::Contention);
        store.fail_tran_type_cursor(0, PortError::Contention);
        let page = block_on(list(&mut store, browse("08", BACKWARD))).expect("the page");
        let first_seven = ["01", "02", "03", "04", "05", "06", "07"];
        assert_eq!(codes(&page.items), first_seven);
        assert_eq!((page.next_key, page.prev_key), (key("08"), key("01")));
        assert_eq!(page.message, None);
    }

    #[test]
    fn an_inline_update_stores_the_trimmed_description_in_the_master_only() {
        // P-COTRTLIC-009, P-COTRTLIC-013, P-COTRTLIC-017, V-COTRTLIC-012
        // (E-73: a change of case alone is stored under the no-change text).
        let updated = HIGHLIGHTED_ROW_WAS_UPDATED;
        let no_change = NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES;
        let cases = [
            ("Retail purchase", "Retail purchase", updated),
            ("  RETAIL 2  ", "RETAIL 2", updated),
            ("PURCHASES", "PURCHASES", updated),
            ("purchase", "purchase", no_change),
            ("  PURCHASE", "PURCHASE", no_change),
        ];
        for (entered, stored, message) in cases {
            let mut store = seeded();
            let outcome = block_on(update_description(&mut store, code("01"), entered));
            let expected = TranTypeUpdated {
                transaction_type: master("01", stored),
                message,
                info: HIGHLIGHTED_ROW_WAS_UPDATED,
            };
            assert_eq!(outcome, Ok(expected), "{entered:?}");
            assert_eq!(store.tran_types[&code("01")], master("01", stored));
            assert_eq!(store.calls, ["read_tran_type", "update_tran_type"]);
            assert!(store.tran_type_file.is_empty(), "no processing copy write");
        }
    }

    #[test]
    fn an_inline_update_rejects_a_missing_or_non_alphanumeric_description() {
        // V-COTRTLIC-013, V-COTRTLIC-014, P-COTRTLIC-013, P-COTRTLIC-021.
        let label = FIELD_TRANSACTION_DESC;
        let missing = DomainError::empty(field_edit_message(label, MUST_BE_SUPPLIED));
        let not_alphanumeric = field_edit_message(label, CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY);
        let invalid = DomainError::invalid(not_alphanumeric);
        let low_values = "\0".repeat(50);
        // Texts whose last character the 50-byte receive splits: the field
        // keeps its lead bytes, which are no letter, digit or space.
        let split_two = format!("{}\u{e9}", "A".repeat(49));
        let split_four = format!("{}\u{1f600}", "A".repeat(48));
        let cases = [
            ("01", "", &missing),
            ("01", "     ", &missing),
            ("01", "*", &missing),
            ("01", "*   ", &missing),
            ("01", low_values.as_str(), &missing),
            ("99", "", &missing),
            ("01", "PURCHASE-RETAIL", &invalid),
            ("01", " *", &invalid),
            ("01", "Caf\u{e9}", &invalid),
            ("01", "Espa\u{f1}a", &invalid),
            ("01", "\u{1f600}", &invalid),
            ("01", "PURCHASE \u{20ac}", &invalid),
            ("01", split_two.as_str(), &invalid),
            ("01", split_four.as_str(), &invalid),
            ("99", "Caf\u{e9}", &invalid),
            ("01", "PAY\0", &invalid),
        ];
        for (type_code, entered, expected) in cases {
            let mut store = seeded();
            let outcome = block_on(update_description(&mut store, code(type_code), entered));
            assert_eq!(outcome.as_ref(), Err(expected), "{type_code} {entered:?}");
            assert_eq!(store.calls, ["read_tran_type"], "nothing is written");
            assert_eq!(store.tran_types[&code("01")], master("01", "PURCHASE"));
        }
    }

    /// Runs the inline update of `entered` on type `type_code` of the seeded
    /// master, with `failure` queued, and checks it fails with `expected` and
    /// leaves type 01 as loaded.
    fn check_failed_update(
        type_code: &str,
        entered: &str,
        failure: Option<(&'static str, PortError)>,
        expected: &DomainError,
    ) {
        let mut store = seeded();
        let case = format!("{type_code} {entered:?} {failure:?}");
        if let Some((method, error)) = failure {
            store.fail_next(method, error);
        }
        let outcome = block_on(update_description(&mut store, code(type_code), entered));
        assert_eq!(outcome.as_ref(), Err(expected), "{case}");
        assert_eq!(store.tran_types[&code("01")], master("01", "PURCHASE"));
    }

    #[test]
    fn a_failed_inline_update_answers_its_db2_text_or_keeps_the_no_change_text() {
        // V-COTRTLIC-016, V-COTRTLIC-017, V-COTRTLIC-018, V-COTRTLIC-012,
        // P-COTRTLIC-010 (E-50: a missing row is NotFound, the rest
        // Unexpected).
        const READ: &str = "read_tran_type";
        const UPDATE: &str = "update_tran_type";
        let unavailable = PortError::Unavailable("pool timed out".to_owned());
        let deleted = db2_text(RECORD_NOT_FOUND_DELETED_BY_OTHERS, 100);
        let deleted = DomainError::not_found(deleted);
        let deadlock = DomainError::unexpected(db2_text(DEADLOCK_SOMEONE_ELSE_UPDATING, -911));
        let failed = |sqlcode| DomainError::unexpected(db2_text(UPDATE_FAILED_WITH, sqlcode));
        let unexpected = DomainError::unexpected(UNEXPECTED_ERROR);

        // A type deleted since it was listed: the update finds no row.
        check_failed_update("99", "NEW", None, &deleted);
        let update_failures = [
            (PortError::NotFound, &deleted),
            (PortError::Contention, &deadlock),
            (PortError::Duplicate, &failed(-803)),
            (PortError::Restricted, &failed(-532)),
            (unavailable.clone(), &unexpected),
        ];
        for (error, expected) in update_failures {
            check_failed_update("01", "NEW", Some((UPDATE, error)), expected);
        }

        // After the no-change text the failure keeps that text.
        let no_change = NO_CHANGE_DETECTED_WITH_RESPECT_TO_DATABASE_VALUES;
        let kept = [
            (PortError::Contention, DomainError::unexpected(no_change)),
            (PortError::NotFound, DomainError::not_found(no_change)),
        ];
        for (error, expected) in kept {
            check_failed_update("01", "purchase", Some((UPDATE, error)), &expected);
        }

        // The read of the listed description fails as the update would.
        check_failed_update("01", "NEW", Some((READ, PortError::Contention)), &deadlock);
        check_failed_update("01", "", Some((READ, unavailable)), &unexpected);
    }

    #[test]
    fn a_delete_answers_the_cotrtlic_texts() {
        // P-COTRTLIC-007, P-COTRTLIC-008, V-COTRTLIC-021, V-COTRTLIC-022,
        // V-COTRTLIC-023 (E-50: the child-record conflict is Conflict).
        let mut store = seeded();
        let outcome = block_on(delete(&mut store, code("07")));
        let expected = TranTypeDeleted {
            type_code: code("07"),
            message: HIGHLIGHTED_ROW_DELETED_HIT_ENTER_TO_CONTINUE,
        };
        assert_eq!(outcome, Ok(expected));
        assert!(!store.tran_types.contains_key(&code("07")));
        assert_eq!(store.calls, ["delete_tran_type"]);

        let children = db2_text(PLEASE_DELETE_ASSOCIATED_CHILD_RECORDS_FIRST, -532);
        let failed = |sqlcode| db2_text(DELETE_FAILED_WITH_MESSAGE, sqlcode);
        let unexpected = DomainError::unexpected(UNEXPECTED_ERROR);
        let unavailable = PortError::Unavailable("gone".to_owned());
        let cases = [
            ("01", None, DomainError::conflict(children)),
            ("99", None, DomainError::not_found(failed(100))),
            ("02", Some(PortError::Contention), {
                DomainError::unexpected(failed(-911))
            }),
            ("02", Some(unavailable), unexpected),
        ];
        for (type_code, failure, expected) in cases {
            let mut store = seeded();
            let case = format!("{type_code} {failure:?}");
            if let Some(error) = failure {
                store.fail_next("delete_tran_type", error);
            }
            let outcome = block_on(delete(&mut store, code(type_code)));
            assert_eq!(outcome, Err(expected), "{case}");
            assert_eq!(store.tran_types.len(), DB2_TYPES.len(), "{case}");
        }
    }

    #[test]
    fn the_rule_futures_are_send_for_a_send_store() {
        // The API serves the rules on a multi-threaded runtime, which needs
        // every future it holds across an await to be Send.
        fn require_send<F: Future + Send>(future: F) -> F {
            future
        }
        let mut store = seeded();
        let page = block_on(require_send(list(&mut store, TranTypeListInput::default())));
        assert!(page.is_ok());
        let updated = block_on(require_send(update_description(
            &mut store,
            code("02"),
            "PAYMENT 2",
        )));
        assert!(updated.is_ok());
        let deleted = block_on(require_send(delete(&mut store, code("02"))));
        assert!(deleted.is_ok());
    }
}
