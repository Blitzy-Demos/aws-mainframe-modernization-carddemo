//! Endpoint 11, `GET /api/v1/transactions`: the transaction list of COTRN00C (transaction
//! `CT00`, map `COTRN0A`; AAP 0.3.4.1). ENTER searches from a start ID, F7 and F8 page, and the
//! `S` selection navigates to COTRN01 on the client without a request.
//!
//! # The paging state travels with the request
//!
//! COTRN00C decides every page from its COMMAREA (`CDEMO-CT00-INFO`) and the key pressed, so a
//! single page key and a direction cannot reproduce it: F8 browses from the last key and F7
//! from the first, a page that does not fill its first or last row keeps the key of an earlier
//! page, F7 tests the page number and F8 the next-page flag (COTRN00C-015 to COTRN00C-026,
//! X-COTRN00C-1). [`TranListResponse`] therefore returns the state (`first_key`, `last_key`,
//! `page_no`, `has_next_page`) and the screen echoes it unchanged in [`TranListQuery`] under
//! the same names, both keys in place of a single `page_key`, with the key it sends:
//! `direction` for F7 and F8, `entry_key` on the request that brings the screen up, neither for
//! ENTER. The state grants nothing, because every request re-reads `transactions`.
//!
//! After a 400 or a 500 the body is a `ScreenError`, which has no room for the state. The
//! screen keeps the state it echoed, with `page_no` 0 after ENTER or a first entry and
//! `has_next_page` true after F7, as `cotrn00c::TranListState::after_failure` states
//! (COTRN00C-013, COTRN00C-027).
//!
//! # Rows
//!
//! A page that was read holds exactly `page_size` slots, row 1 first, with `null` for a blank
//! row: a short forward page ends in blank rows and a short backward page starts with them
//! (COTRN00C-018, COTRN00C-026). The F7 and F8 boundary answers read nothing and set
//! `rows_kept`: the screen keeps the rows it shows (COTRN00C-021, COTRN00C-022).
//!
//! A row carries `TRAN-ORIG-TS` typed and beside it the `MM/DD/YY` display date the domain
//! builds (COTRN00C-029; Typed Dates and Timestamps), the amount as the 12 characters of
//! `PIC +99999999.99` (COTRN00C-032; R1), and no card number in any form (COTRN00C-034;
//! Cardholder Data Redaction).

use chrono::NaiveDateTime;
use domain::copybook::{PicU, PicX};
use domain::online::cotrn00c as rules;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{PageDirection, text};

/// The key of the transfer that brought the screen up, sent with the first request only
/// (COTRN00C-003, COTRN00C-004, X-COTRN00C-2).
// The schema name `TranListEntryKey` is fixed (Generated API Types Only); it is unique across
// the API because every module's schemas share one `components.schemas` map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TranListEntryKey {
    /// `enter`: the main menu's option. The page starts at the lowest transaction.
    Enter,
    /// `pf3`: COTRN01's F3 back to the list, read as `enter`.
    Pf3,
    /// `pf5`: COTRN01's F5 to the list. The program's skip read drops the lowest transaction.
    Pf5,
}

impl From<TranListEntryKey> for rules::EntryKey {
    fn from(key: TranListEntryKey) -> Self {
        match key {
            TranListEntryKey::Enter => Self::Enter,
            TranListEntryKey::Pf3 => Self::Pf3,
            TranListEntryKey::Pf5 => Self::Pf5,
        }
    }
}

/// The query string of endpoint 11: the key sent, the search value and the echoed paging
/// state. Every member is optional; with none the request is an ENTER with a blank search over
/// the state a menu entry starts from (COTRN00C-002, COTRN00C-011).
// The schema name `TranListQuery` is fixed (Generated API Types Only).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, IntoParams, Validate,
)]
#[into_params(parameter_in = Query)]
#[schema(example = json!({
    "direction": "forward",
    "first_key": "0000000000000001",
    "last_key": "0000000000000010",
    "page_no": 1,
    "has_next_page": true
}))]
pub struct TranListQuery {
    /// `TRNIDIN`, the search value as typed, at most the 16 characters of the map field
    /// (COTRN00-015). Blank starts at the lowest ID; anything else but 16 digits is answered 400
    /// `Tran ID must be Numeric ...` by the program's own edit (COTRN00C-011 to COTRN00C-013).
    /// Read on ENTER only.
    #[validate(length(max = 16))]
    #[schema(max_length = 16)]
    #[param(max_length = 16)]
    pub start_tran_id: Option<String>,
    /// F8 (`forward`) or F7 (`backward`); absent for ENTER and for a first entry.
    pub direction: Option<PageDirection>,
    /// The transfer key of the first request after the screen is brought up; when present it
    /// decides the request, and `direction` and `start_tran_id` are not read.
    pub entry_key: Option<TranListEntryKey>,
    /// `CDEMO-CT00-TRNID-FIRST`, echoed from the last response.
    #[validate(length(max = 16))]
    #[schema(max_length = 16)]
    #[param(max_length = 16)]
    pub first_key: Option<String>,
    /// `CDEMO-CT00-TRNID-LAST`, echoed from the last response.
    #[validate(length(max = 16))]
    #[schema(max_length = 16)]
    #[param(max_length = 16)]
    pub last_key: Option<String>,
    /// `CDEMO-CT00-PAGE-NUM PIC 9(08)`, echoed from the last response; absent is 0. Page 0 is a
    /// real state, after a first entry that showed no row or a rejected search (COTRN00C-004,
    /// COTRN00C-013), so 0 is admitted.
    // Bounds of `PIC 9(08)`: 0 to eight nines.
    #[validate(range(min = 0, max = 99_999_999))]
    #[schema(minimum = 0, maximum = 99_999_999)]
    #[param(minimum = 0, maximum = 99_999_999)]
    pub page_no: Option<i64>,
    /// `CDEMO-CT00-NEXT-PAGE-FLG`, echoed from the last response: `true` is `Y`; absent or
    /// `false` is any other value.
    pub has_next_page: Option<bool>,
    /// Rows of a page, 1 to 100, default 10, the rows of `COTRN0A` (Bounded Pagination); 422
    /// outside that range. The screen never sends it.
    #[validate(range(min = 1, max = 100))]
    #[schema(minimum = 1, maximum = 100)]
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

impl TranListQuery {
    /// The key the request was sent with: the entry key when present, otherwise F8 for
    /// `forward`, F7 for `backward` and ENTER without a direction.
    #[must_use]
    pub fn key(&self) -> rules::ListKey {
        match (self.entry_key, self.direction.map(rules::Direction::from)) {
            (Some(entry), _) => rules::ListKey::Entry(entry.into()),
            (None, None) => rules::ListKey::Enter,
            (None, Some(rules::Direction::Forward)) => rules::ListKey::Pf8,
            (None, Some(rules::Direction::Backward)) => rules::ListKey::Pf7,
        }
    }

    /// The echoed state as the COMMAREA holds it.
    ///
    /// Each key is moved into its `PIC X(16)` field as an alphanumeric `MOVE` does, padded with
    /// spaces, and a blank key is `None`. A key the response sent comes back byte for byte, and
    /// any other text only starts a browse elsewhere, since the state grants nothing.
    #[must_use]
    pub fn state(&self) -> rules::TranListState {
        // TRUNCATION: checked, never lossy: validation admits only 0 to 99,999,999, which
        // `PIC 9(08)` holds, and a value the conversion refuses is the blank page number 0.
        let page_no = self
            .page_no
            .and_then(|page| PicU::try_from(page).ok())
            .unwrap_or(PicU::ZERO);
        let next_page = match self.has_next_page {
            Some(true) => rules::NextPage::Yes,
            Some(false) | None => rules::NextPage::No,
        };
        rules::TranListState::new(
            self.first_key.as_deref().map(key_field),
            self.last_key.as_deref().map(key_field),
            page_no,
            next_page,
        )
    }

    /// The rows of the page: `page_size`, or the screen's 10 when it is absent.
    #[must_use]
    pub fn page_size(&self) -> usize {
        // TRUNCATION: checked both ways, never lossy: the screen size 10 fits `u32`, validation
        // admits only 1 to 100, and a conversion that fails gives the screen size.
        u32::try_from(rules::DEFAULT_PAGE_SIZE)
            .ok()
            .map(|default| super::resolve_page_size(self.page_size, default))
            .and_then(|rows| usize::try_from(rows).ok())
            .unwrap_or(rules::DEFAULT_PAGE_SIZE)
    }
}

/// A `PIC X(16)` key field holding `key` as an alphanumeric `MOVE` leaves it.
fn key_field(key: &str) -> PicX<16> {
    PicX::moved_from(key.as_bytes())
}

/// One list row of `COTRN0A`, rows 1 to 10 at screen lines 10 to 19 (COTRN00C-028 to
/// COTRN00C-034).
// The schema name `TranListRow` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "tran_id": "0000000000000001",
    "orig_ts": "2022-07-18T13:05:09.120",
    "display_date": "07/18/22",
    "description": "Purchase at Example Store",
    "amount": "+00000050.47"
}))]
pub struct TranListRow {
    /// `TRNIDnn`: the stored `TRAN-ID`, 16 characters, which the `S` selection carries to
    /// COTRN01 (COTRN00C-028).
    pub tran_id: String,
    /// `TRAN-ORIG-TS` as its typed timestamp, `null` when the field is blank (COTRN00C-029;
    /// Typed Dates and Timestamps).
    pub orig_ts: Option<NaiveDateTime>,
    /// `TDATEnn`: the `MM/DD/YY` date the screen shows, built by the program's rule from
    /// `orig_ts`, so the screen formats nothing; `null` for a blank `TRAN-ORIG-TS`, whose COBOL
    /// display no source fixes (COTRN00C-029, COTRN00C-030).
    #[schema(min_length = 8, max_length = 8)]
    pub display_date: Option<String>,
    /// `TDESCnn`: the first 26 bytes of `TRAN-DESC`, without trailing spaces (COTRN00C-031).
    #[schema(max_length = 26)]
    pub description: String,
    /// `TAMTnnn`: `TRAN-AMT` edited by `PIC +99999999.99`, 12 characters such as
    /// `+00000050.47` (COTRN00C-032).
    #[schema(min_length = 12, max_length = 12)]
    pub amount: String,
}

impl From<rules::TranListRow> for TranListRow {
    fn from(row: rules::TranListRow) -> Self {
        Self {
            tran_id: text(row.tran_id()),
            orig_ts: row.orig_ts(),
            display_date: row.display_date().map(text),
            description: text(row.description()),
            amount: row.amount().to_owned(),
        }
    }
}

/// The 200 of endpoint 11: the rows, the paging state the screen echoes with its next request
/// and the message the program moved last (COTRN00C-035).
// The schema name `TranListResponse` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "rows": [
        {
            "tran_id": "0000000000000001",
            "orig_ts": "2022-07-18T13:05:09.120",
            "display_date": "07/18/22",
            "description": "Purchase at Example Store",
            "amount": "+00000050.47"
        },
        null
    ],
    "rows_kept": false,
    "first_key": "0000000000000001",
    "last_key": null,
    "page_no": 1,
    "has_next_page": false,
    "message": "You have reached the bottom of the page..."
}))]
pub struct TranListResponse {
    /// The page read: exactly `page_size` slots, row 1 first, `null` for a blank row. Empty
    /// when `rows_kept` is true.
    pub rows: Vec<Option<TranListRow>>,
    /// True when nothing was read: F7 at the top or F8 at the bottom, where the screen keeps
    /// the rows, page number, search value and selectors it shows (COTRN00C-021,
    /// COTRN00C-022).
    pub rows_kept: bool,
    /// `CDEMO-CT00-TRNID-FIRST`, `null` when blank: echo it as `first_key`.
    pub first_key: Option<String>,
    /// `CDEMO-CT00-TRNID-LAST`, `null` when blank: echo it as `last_key`.
    pub last_key: Option<String>,
    /// `CDEMO-CT00-PAGE-NUM PIC 9(08)`: echo it as `page_no`. The screen shows it as eight
    /// digits with leading zeros, and only after a page was read (COTRN00C-036).
    #[schema(maximum = 99_999_999)]
    pub page_no: u64,
    /// `CDEMO-CT00-NEXT-PAGE-FLG` is `Y`: echo it as `has_next_page`.
    pub has_next_page: bool,
    /// The message row, `null` when the program moved none, such as `You have reached the
    /// bottom of the page...` (COTRN00C-017) or `You are already at the top of the page...`
    /// (COTRN00C-022).
    pub message: Option<String>,
}

impl From<rules::TranPage> for TranListResponse {
    fn from(page: rules::TranPage) -> Self {
        let (rows, state, message) = page.into_parts();
        let (rows, rows_kept) = match rows {
            rules::PageRows::Read(slots) => (
                slots
                    .into_iter()
                    .map(|slot| slot.map(TranListRow::from))
                    .collect(),
                false,
            ),
            rules::PageRows::Kept => (Vec::new(), true),
        };
        Self {
            rows,
            rows_kept,
            first_key: state.first_key().map(text),
            last_key: state.last_key().map(text),
            page_no: state.page_no().get(),
            has_next_page: state.next_page() == rules::NextPage::Yes,
            message: message.map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::FromRequestParts;
    use axum::http::{Request, StatusCode};
    use chrono::{NaiveDate, NaiveDateTime};
    use domain::copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
    use domain::money::{Decimal, Money};
    use domain::online::cotrn00c as rules;
    use domain::records::transaction::TransactionRecord;
    use domain::sensitive::CardNumber;
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{TranListEntryKey, TranListQuery, TranListResponse, TranListRow};
    use crate::dto::{
        FIELD_LENGTH_MESSAGE, FIELD_RANGE_MESSAGE, INVALID_QUERY, PAGE_SIZE_FIELD,
        PAGE_SIZE_MESSAGE, PageDirection, ValidQuery,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A synthetic card number, in no fixture; no response may carry it.
    const CARD: &str = "4000001234567899";

    /// The instant every synthetic transaction originates at.
    fn origin() -> Result<NaiveDateTime, String> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_micro_opt(13, 5, 9, 120_000))
            .ok_or_else(|| "no such instant".to_owned())
    }

    /// The transaction ID `n`, sixteen digits with leading zeros.
    fn id(n: u64) -> Result<PicX<16>, String> {
        PicX::from_text(&format!("{n:016}")).map_err(shown)
    }

    /// The `Debug` text of `error`, for a test failure message.
    fn shown<E: core::fmt::Debug>(error: E) -> String {
        format!("{error:?}")
    }

    /// Transaction `n` of `cents` hundredths, described `desc`, carrying [`CARD`].
    fn record(n: u64, cents: i64, desc: &str) -> Result<TransactionRecord, String> {
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
            card_num: CardNumber::new(PicX::from_text(CARD).map_err(shown)?),
            orig_ts: PicTimestamp::new(Some(
                CobolTimestamp::new(origin()?, TsForm::Iso).map_err(shown)?,
            )),
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// The list rows of transactions `ids`, each of 50.47.
    fn rows(ids: impl IntoIterator<Item = u64>) -> Result<Vec<rules::TranListRow>, String> {
        ids.into_iter()
            .map(|n| Ok(rules::row_from(&record(n, 5047, "Purchase")?)))
            .collect()
    }

    /// The response to `query` over the records `read`, through the program's rules.
    fn respond(
        query: &TranListQuery,
        read: Vec<rules::TranListRow>,
    ) -> Result<TranListResponse, String> {
        let plan = rules::plan(
            query.key(),
            query.start_tran_id.as_deref(),
            query.state(),
            query.page_size(),
        )
        .map_err(|failure| failure.to_string())?;
        let page = match plan {
            rules::Plan::Read(browse) => {
                rules::page(browse, read).map_err(|failure| failure.to_string())?
            }
            rules::Plan::Answer(page) => page,
        };
        Ok(TranListResponse::from(page))
    }

    /// The query a screen sends with `direction` after `response`, its state echoed.
    fn echo(response: &TranListResponse, direction: Option<PageDirection>) -> TranListQuery {
        TranListQuery {
            direction,
            first_key: response.first_key.clone(),
            last_key: response.last_key.clone(),
            // TRUNCATION: checked, never lossy. The page number is PIC 9(08), at most
            // 99,999,999, which `i64` holds; `.ok()` would drop only an impossible value to
            // absent, never wrap it.
            page_no: i64::try_from(response.page_no).ok(),
            has_next_page: Some(response.has_next_page),
            ..TranListQuery::default()
        }
    }

    /// The query string `uri` through the endpoint's extractor.
    async fn extract(uri: &str) -> Result<Result<TranListQuery, ScreenError>, Box<dyn StdError>> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<TranListQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(query)| query),
        )
    }

    /// The IDs on the response's rows, row 1 first, `None` for a blank row.
    fn ids_on(response: &TranListResponse) -> Vec<Option<&str>> {
        response
            .rows
            .iter()
            .map(|slot| slot.as_ref().map(|row| row.tran_id.as_str()))
            .collect()
    }

    #[test]
    fn the_entry_key_decides_then_the_direction_then_enter() {
        use PageDirection::{Backward, Forward};
        use rules::{EntryKey, ListKey};
        let cases = [
            (None, None, ListKey::Enter),
            (Some(Forward), None, ListKey::Pf8),
            (Some(Backward), None, ListKey::Pf7),
            (
                None,
                Some(TranListEntryKey::Enter),
                ListKey::Entry(EntryKey::Enter),
            ),
            (
                None,
                Some(TranListEntryKey::Pf3),
                ListKey::Entry(EntryKey::Pf3),
            ),
            (
                Some(Forward),
                Some(TranListEntryKey::Pf5),
                ListKey::Entry(EntryKey::Pf5),
            ),
            (
                Some(Backward),
                Some(TranListEntryKey::Enter),
                ListKey::Entry(EntryKey::Enter),
            ),
        ];
        for (direction, entry_key, key) in cases {
            let query = TranListQuery {
                direction,
                entry_key,
                ..TranListQuery::default()
            };
            assert_eq!(query.key(), key, "{direction:?} {entry_key:?}");
        }
    }

    #[test]
    fn an_empty_query_is_an_enter_over_the_menu_entry_state() {
        let query = TranListQuery::default();
        assert_eq!(query.key(), rules::ListKey::Enter);
        assert_eq!(query.state(), rules::TranListState::default());
        assert_eq!(query.page_size(), rules::DEFAULT_PAGE_SIZE);
        for blank in ["", "   ", "                "] {
            let query = TranListQuery {
                first_key: Some(blank.to_owned()),
                last_key: Some(blank.to_owned()),
                has_next_page: Some(false),
                page_no: Some(0),
                ..TranListQuery::default()
            };
            assert_eq!(query.state(), rules::TranListState::default(), "{blank:?}");
        }
    }

    #[test]
    fn page_size_is_the_request_or_the_ten_screen_rows() {
        for (requested, rows) in [(None, 10), (Some(1), 1), (Some(25), 25), (Some(100), 100)] {
            let query = TranListQuery {
                page_size: requested,
                ..TranListQuery::default()
            };
            assert_eq!(query.page_size(), rows, "{requested:?}");
        }
    }

    #[test]
    fn the_echoed_state_is_the_state_the_response_returned() -> TestResult {
        // ENTER over eleven records: a full first page and a read-ahead record.
        let first = respond(&TranListQuery::default(), rows(1..=11)?)?;
        assert_eq!(first.first_key.as_deref(), Some("0000000000000001"));
        assert_eq!(first.last_key.as_deref(), Some("0000000000000010"));
        assert_eq!((first.page_no, first.has_next_page), (1, true));
        assert_eq!(first.message, None);

        let state = echo(&first, Some(PageDirection::Forward)).state();
        let expected = rules::TranListState::new(
            Some(id(1)?),
            Some(id(10)?),
            PicU::new(1)?,
            rules::NextPage::Yes,
        );
        assert_eq!(state, expected);

        // F8 from that state: the skip read drops record 10, then records 11 to 13.
        let second = respond(&echo(&first, Some(PageDirection::Forward)), rows(10..=13)?)?;
        let mut expected_ids = vec![
            Some("0000000000000011"),
            Some("0000000000000012"),
            Some("0000000000000013"),
        ];
        expected_ids.resize(10, None);
        assert_eq!(ids_on(&second), expected_ids);
        assert_eq!(second.first_key.as_deref(), Some("0000000000000011"));
        // A short page keeps the last key of the page before it.
        assert_eq!(second.last_key.as_deref(), Some("0000000000000010"));
        assert_eq!((second.page_no, second.has_next_page), (2, false));
        assert_eq!(
            second.message.as_deref(),
            Some("You have reached the bottom of the page...")
        );
        Ok(())
    }

    #[test]
    fn a_short_backward_page_starts_with_blank_rows() -> TestResult {
        // F7 from page 2 whose first key is record 3: the skip read drops record 3, and the two
        // records before it fill rows 9 and 10.
        let query = TranListQuery {
            direction: Some(PageDirection::Backward),
            first_key: Some("0000000000000003".to_owned()),
            last_key: Some("0000000000000012".to_owned()),
            page_no: Some(2),
            has_next_page: Some(true),
            ..TranListQuery::default()
        };
        let response = respond(&query, rows([3, 2, 1])?)?;
        let mut expected_ids = vec![None; 8];
        expected_ids.extend([Some("0000000000000001"), Some("0000000000000002")]);
        assert_eq!(ids_on(&response), expected_ids);
        assert!(!response.rows_kept);
        assert_eq!(response.first_key.as_deref(), Some("0000000000000003"));
        assert_eq!(response.last_key.as_deref(), Some("0000000000000002"));
        assert_eq!((response.page_no, response.has_next_page), (2, true));
        assert_eq!(
            response.message.as_deref(),
            Some("You have reached the top of the page...")
        );
        Ok(())
    }

    #[test]
    fn a_boundary_answer_keeps_the_rows_and_the_state() -> TestResult {
        let at_bottom = TranListQuery {
            direction: Some(PageDirection::Forward),
            first_key: Some("0000000000000001".to_owned()),
            last_key: Some("0000000000000010".to_owned()),
            page_no: Some(1),
            has_next_page: Some(false),
            ..TranListQuery::default()
        };
        let response = respond(&at_bottom, Vec::new())?;
        assert!(response.rows_kept);
        assert!(response.rows.is_empty());
        assert_eq!(
            response.message.as_deref(),
            Some("You are already at the bottom of the page...")
        );
        assert_eq!(echo(&response, None).state(), at_bottom.state());

        // F7 at page 0 answers at the top and sets the next-page flag.
        let at_top = TranListQuery {
            direction: Some(PageDirection::Backward),
            page_no: Some(0),
            ..TranListQuery::default()
        };
        let response = respond(&at_top, Vec::new())?;
        assert!(response.rows_kept);
        assert_eq!((response.page_no, response.has_next_page), (0, true));
        assert_eq!(
            response.message.as_deref(),
            Some("You are already at the top of the page...")
        );
        Ok(())
    }

    #[test]
    fn a_row_carries_the_screen_fields_and_no_card_number() -> TestResult {
        let long = "Purchase at a store whose name runs past the screen";
        let row = TranListRow::from(rules::row_from(&record(7, -123_456, long)?));
        assert_eq!(
            row,
            TranListRow {
                tran_id: "0000000000000007".to_owned(),
                orig_ts: Some(origin()?),
                display_date: Some("07/18/22".to_owned()),
                description: "Purchase at a store whose".to_owned(),
                amount: "-00001234.56".to_owned(),
            }
        );
        let json = serde_json::to_value(&row)?;
        assert_eq!(
            json,
            serde_json::json!({
                "tran_id": "0000000000000007",
                "orig_ts": "2022-07-18T13:05:09.120",
                "display_date": "07/18/22",
                "description": "Purchase at a store whose",
                "amount": "-00001234.56"
            })
        );

        let response = respond(&TranListQuery::default(), rows([1])?)?;
        let body = serde_json::to_string(&response)?;
        assert!(!body.contains(CARD) && !body.contains("7899"), "{body}");
        assert!(!body.contains("card"), "{body}");
        assert_eq!(serde_json::from_str::<TranListResponse>(&body)?, response);
        Ok(())
    }

    #[tokio::test]
    async fn the_query_string_is_parsed_with_its_state() -> TestResult {
        let query = extract(
            "/api/v1/transactions?direction=backward&first_key=0000000000000011\
             &last_key=0000000000000020&page_no=2&has_next_page=true&page_size=7",
        )
        .await?
        .map_err(|error| error.to_string())?;
        assert_eq!(query.key(), rules::ListKey::Pf7);
        assert_eq!(query.page_size(), 7);
        assert_eq!(
            query.state(),
            rules::TranListState::new(
                Some(id(11)?),
                Some(id(20)?),
                PicU::new(2)?,
                rules::NextPage::Yes
            )
        );

        let query = extract("/api/v1/transactions?entry_key=pf5&start_tran_id=ABC&page_no=0")
            .await?
            .map_err(|error| error.to_string())?;
        assert_eq!(query.key(), rules::ListKey::Entry(rules::EntryKey::Pf5));
        assert_eq!(query.start_tran_id.as_deref(), Some("ABC"));
        Ok(())
    }

    #[tokio::test]
    async fn a_query_outside_its_rules_is_rejected() -> TestResult {
        let too_long = "0".repeat(17);
        let cases = [
            (
                "/api/v1/transactions?page_size=101".to_owned(),
                StatusCode::UNPROCESSABLE_ENTITY,
                PAGE_SIZE_MESSAGE,
                Some(PAGE_SIZE_FIELD),
            ),
            (
                "/api/v1/transactions?page_size=0&start_tran_id=ABC".to_owned(),
                StatusCode::UNPROCESSABLE_ENTITY,
                PAGE_SIZE_MESSAGE,
                Some(PAGE_SIZE_FIELD),
            ),
            (
                format!("/api/v1/transactions?start_tran_id={too_long}"),
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("start_tran_id"),
            ),
            (
                format!("/api/v1/transactions?first_key={too_long}"),
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("first_key"),
            ),
            (
                format!("/api/v1/transactions?last_key={too_long}"),
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("last_key"),
            ),
            (
                "/api/v1/transactions?page_no=-1".to_owned(),
                StatusCode::BAD_REQUEST,
                FIELD_RANGE_MESSAGE,
                Some("page_no"),
            ),
            (
                "/api/v1/transactions?page_no=100000000".to_owned(),
                StatusCode::BAD_REQUEST,
                FIELD_RANGE_MESSAGE,
                Some("page_no"),
            ),
            (
                "/api/v1/transactions?entry_key=pf7".to_owned(),
                StatusCode::BAD_REQUEST,
                INVALID_QUERY,
                None,
            ),
            (
                "/api/v1/transactions?has_next_page=Y".to_owned(),
                StatusCode::BAD_REQUEST,
                INVALID_QUERY,
                None,
            ),
        ];
        for (uri, status, message, field) in cases {
            let error = extract(&uri).await?.err().ok_or("accepted")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (status, message, field),
                "{uri}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_schemas_carry_their_contract_names_and_types() -> TestResult {
        assert_eq!(TranListQuery::name(), "TranListQuery");
        assert_eq!(TranListRow::name(), "TranListRow");
        assert_eq!(TranListResponse::name(), "TranListResponse");
        assert_eq!(TranListEntryKey::name(), "TranListEntryKey");

        let row = serde_json::to_value(<TranListRow as PartialSchema>::schema())?;
        assert_eq!(row["properties"]["orig_ts"]["format"], "date-time", "{row}");
        assert_eq!(row["properties"]["amount"]["maxLength"], 12, "{row}");

        let params = serde_json::to_value(TranListQuery::into_params(|| None))?;
        let names: Vec<&str> = params
            .as_array()
            .ok_or("no parameter list")?
            .iter()
            .filter_map(|param| {
                assert_eq!(param["in"], "query", "{param}");
                param["name"].as_str()
            })
            .collect();
        assert_eq!(
            names,
            [
                "start_tran_id",
                "direction",
                "entry_key",
                "first_key",
                "last_key",
                "page_no",
                "has_next_page",
                "page_size"
            ]
        );
        let page_size = params
            .as_array()
            .and_then(|list| list.iter().find(|param| param["name"] == "page_size"))
            .ok_or("no page_size parameter")?;
        assert_eq!(page_size["schema"]["maximum"], 100, "{page_size}");
        assert_eq!(page_size["schema"]["minimum"], 1, "{page_size}");
        Ok(())
    }
}
