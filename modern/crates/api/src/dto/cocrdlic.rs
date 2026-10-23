//! The contract of endpoint 7, `GET /api/v1/cards`: the card list of COCRDLIC (transaction
//! `CCLI`, map `CCRDLIA`, route `/cards`; AAP 0.3.4.1).
//!
//! ENTER sends the two filters and F7 and F8 page. A row selection (`S` or `U`) never reaches
//! the endpoint: the screen navigates to `/cards/view` or `/cards/update` with the row's
//! `acct_id` and `card_ref` (COCRDLIC-015). Every edit, read decision and message belongs to
//! `domain::online::cocrdlic`; this module holds the wire types, their `validator` rules (the
//! server half of the map `LENGTH` rows `COCRDLI-015` and `COCRDLI-018`) and the conversions
//! between them and the domain.
//!
//! # Paging state
//!
//! COCRDLIC keeps its paging state in the COMMAREA between interactions: the page number, the
//! first and last keys, the next-page indicator and the last-page flag (`COCRDLIC.cbl:229-248`).
//! The endpoint is stateless, so [`CardListResponse`] returns that state and the screen sends
//! it back unchanged with its next request, under the same names (`page_no`, `first_key`,
//! `last_key`, `has_next_page`, `last_page_shown`), with the key pressed as `direction` and
//! `reread` set for the selection-error re-read (COCRDLIC-014). `ListState::from(&query)`
//! turns the echo into the state of `domain::online::cocrdlic`; a request that echoes nothing
//! is a fresh start (COCRDLIC-002, COCRDLIC-017). The echo carries no authority: the server
//! re-reads the cards under the filters sent, and an echoed key is only a handle to start the
//! browse from.
//!
//! # Cardholder Data Redaction
//!
//! Rows and keys carry `card_ref` handles, never card numbers (`E32`, COCRDLIC-032). A row's
//! `card_num` is a `CardNumber`, which serialises as twelve `*` and the last four digits. The
//! card filter is request input, not egress: it is never serialised back, its schema is
//! write-only with a redacted example, and the query's `Debug` masks it.
//!
//! # Bounded Pagination
//!
//! `page_size` admits 1 to 100; [`super::ValidQuery`] answers any other value 422. Absent, the
//! service reads `DEFAULT_PAGE_SIZE` (7) rows through [`super::resolve_page_size`]. The screen
//! never sends it, so every paging result of the audit is stated for seven rows.

use core::fmt;

use domain::CardNumber;
use domain::online::cocrdlic as rules;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{PageDirection, digits, full_text};

/// What the query's `Debug` shows for a card filter, whatever was typed: never its digits, nor
/// its length.
const CARD_FILTER_MASK: &str = "****************";

/// The query of endpoint 7: the two filters as typed, the key pressed, and the paging state of
/// the previous response, echoed unchanged.
///
/// Every member is optional. A request with none of them is the fresh start of the list
/// (COCRDLIC-002, COCRDLIC-017): page 1 from the lowest card number, with no filter.
// The schema name `CardListQuery` is the frontend's `components["schemas"]["CardListQuery"]`
// (Generated API Types Only); `api::openapi::ApiDoc` registers it, and the handler declares
// `params(CardListQuery)` because utoipa cannot see through `ValidQuery`.
#[derive(Clone, Default, PartialEq, Eq, Deserialize, ToSchema, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct CardListQuery {
    /// `ACCTSID`, the account filter, as typed (map `LENGTH` 11). Blank, or eleven `0` digits,
    /// is no filter (COCRDLIC-005); eleven digits list only that account's cards (COCRDLIC-010);
    /// anything else is 400 with field `acct_id` (COCRDLIC-006).
    // Raw text: `2210-EDIT-ACCOUNT` owns the numeric edit and its message, so typing the member
    // as an integer would replace that message with a deserialisation error.
    #[validate(length(max = 11))]
    #[schema(max_length = 11, example = "00000000050")]
    #[param(max_length = 11, example = "00000000050")]
    pub acct_id: Option<String>,

    /// `CARDSID`, the card filter, as typed (map `LENGTH` 16). Blank, or sixteen `0` digits, is
    /// no filter (COCRDLIC-007); sixteen digits list only that card (COCRDLIC-010); anything else
    /// is 400 with field `card_num` (COCRDLIC-008). Input only: never sent back.
    #[validate(length(max = 16))]
    #[schema(write_only, max_length = 16, example = "************5740")]
    #[param(write_only, max_length = 16, example = "************5740")]
    pub card_num: Option<String>,

    /// The key pressed: `forward` for F8, `backward` for F7, absent for ENTER or any key the
    /// program processes as ENTER (COCRDLIC-003).
    pub direction: Option<PageDirection>,

    /// The page number of the previous response, 0 to 9 (`PAGENO`, COCRDLIC-030). Absent, page 1.
    /// Outside 0 to 9 it is 400 with field `page_no`.
    // 0 is admitted although no page count starts there: the one-digit page number wraps from 9
    // to 0 (COCRDLIC-030), and a response that showed page 0 must be echoable.
    #[validate(range(min = 0, max = 9))]
    #[schema(minimum = 0, maximum = 9)]
    #[param(minimum = 0, maximum = 9)]
    pub page_no: Option<i64>,

    /// The `first_key` of the previous response: the `card_ref` that ENTER and F7 read from
    /// (COCRDLIC-018, COCRDLIC-026, COCRDLIC-027). Absent, the browse starts below every card.
    // Both keys are echoed in place of the single `page_key` the endpoint table sketches. One key
    // cannot reproduce F8 without a next page, which re-reads from the first key (COCRDLIC-024),
    // nor an F8 page that lists no card, which keeps the previous first key (COCRDLIC-021,
    // COCRDLIC-022). The full echo is the resolution escalation candidate X-COCRDLIC-2 proposes
    // and the shape of `rules::ListState`.
    pub first_key: Option<i64>,

    /// The `last_key` of the previous response: the `card_ref` that F8 reads from while a next
    /// page exists (COCRDLIC-023).
    pub last_key: Option<i64>,

    /// The `has_next_page` of the previous response. Absent, no next page: F8 then re-reads the
    /// page from `first_key` (COCRDLIC-024).
    pub has_next_page: Option<bool>,

    /// The `last_page_shown` of the previous response: whether F8 already showed the last page,
    /// so that a further F8 answers `NO MORE PAGES TO DISPLAY` (COCRDLIC-024, COCRDLIC-025).
    /// Absent, not shown.
    pub last_page_shown: Option<bool>,

    /// `true` for the re-read after a selection error the screen found (COCRDLIC-014): the list
    /// is read again from the start of the card file under the filters, the echoed page number
    /// is kept and the key pressed still decides the messages. Absent, an ordinary request.
    pub reread: Option<bool>,

    /// Rows per page, 1 to 100 (Bounded Pagination); any other value is 422. Absent, 7, the
    /// program's screen size. The screen never sends it.
    #[serde(default, deserialize_with = "super::page_size_from_query")]
    #[validate(range(min = 1, max = 100))]
    #[schema(minimum = 1, maximum = 100)]
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

/// Every member but the card filter, which is request input and is never written back out
/// (Cardholder Data Redaction). An absent member is a JSON `null`, and a query string leaves it
/// out.
// Written by hand rather than derived with `#[serde(skip_serializing)]` on `card_num`: utoipa
// drops a member serde skips from `IntoParams`, which would remove the card filter from the
// operation's parameters and from the frontend's generated types.
impl Serialize for CardListQuery {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("CardListQuery", 9)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("direction", &self.direction)?;
        out.serialize_field("page_no", &self.page_no)?;
        out.serialize_field("first_key", &self.first_key)?;
        out.serialize_field("last_key", &self.last_key)?;
        out.serialize_field("has_next_page", &self.has_next_page)?;
        out.serialize_field("last_page_shown", &self.last_page_shown)?;
        out.serialize_field("reread", &self.reread)?;
        out.serialize_field("page_size", &self.page_size)?;
        out.end()
    }
}

/// Every member, the card filter masked: a typed card number is request input and must reach
/// no log line (Cardholder Data Redaction).
impl fmt::Debug for CardListQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CardListQuery")
            .field("acct_id", &self.acct_id)
            .field(
                "card_num",
                &self.card_num.as_ref().map(|_| CARD_FILTER_MASK),
            )
            .field("direction", &self.direction)
            .field("page_no", &self.page_no)
            .field("first_key", &self.first_key)
            .field("last_key", &self.last_key)
            .field("has_next_page", &self.has_next_page)
            .field("last_page_shown", &self.last_page_shown)
            .field("reread", &self.reread)
            .field("page_size", &self.page_size)
            .finish()
    }
}

/// The paging state a request echoes, as `rules::plan` and `rules::plan_reread` take it. An
/// absent member takes its value in `rules::ListState::fresh()`.
///
/// A page number outside 0 to 9 never reaches this conversion, because `ValidQuery` answers it
/// 400 first; read without validation, it counts as absent. Keys are taken as handles only: a
/// handle that names no card is the service's to resolve.
impl From<&CardListQuery> for rules::ListState {
    fn from(query: &CardListQuery) -> Self {
        let fresh = Self::fresh();
        Self {
            // TRUNCATION: checked, never lossy. Validation admits only 0 to 9, which the one-digit
            // page number holds; a value the conversion refuses counts as absent, never wrapped.
            page_no: query
                .page_no
                .and_then(|page_no| rules::PageNumber::try_from(page_no).ok())
                .unwrap_or(fresh.page_no),
            first_key: query.first_key.map(rules::CardRef::new),
            last_key: query.last_key.map(rules::CardRef::new),
            next_page: if query.has_next_page == Some(true) {
                rules::NextPage::Exists
            } else {
                rules::NextPage::NotExists
            },
            last_page: if query.last_page_shown == Some(true) {
                rules::LastPage::Shown
            } else {
                rules::LastPage::NotShown
            },
        }
    }
}

/// One listed card (COCRDLIC-032): row `n` of the map, `ACCTNOn`, `CRDNUMn` and `CRDSTSn`
/// (`COCRDLI-030` to `COCRDLI-032` for row 1), with the card's handle.
///
/// No card verification code, embossed name or expiry date is listed.
// No `Deserialize`: `card_num` is a `CardNumber`, which serialises only its redacted form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardListRow {
    /// The card's handle, the `cards.card_ref` identity, which a selection carries to
    /// `/cards/view` or `/cards/update` and which endpoints 8 to 10 accept.
    #[schema(example = 1)]
    pub card_ref: i64,

    /// `CRDNUMn`: the card number redacted to twelve `*` and its last four digits, 16
    /// characters; the full number is never sent (`E32`).
    #[schema(value_type = String, example = "************5740")]
    pub card_num: CardNumber,

    /// `ACCTNOn`: the card's account, `CARD-ACCT-ID`, as its eleven digits with leading zeros.
    #[schema(example = "00000000050")]
    pub acct_id: String,

    /// `CRDSTSn`: `CARD-ACTIVE-STATUS`, the stored byte as one character, a space included
    /// (COCRDLIC-032).
    #[schema(example = "Y")]
    pub status: String,
}

impl From<rules::CardListRow> for CardListRow {
    fn from(row: rules::CardListRow) -> Self {
        Self {
            card_ref: row.card_ref.get(),
            card_num: row.card_num,
            acct_id: digits(row.acct_id),
            status: full_text(&row.status),
        }
    }
}

/// One page of the card list: what COCRDLIC's `1000-SEND-MAP` shows after a read, and the
/// paging state the next request echoes (COCRDLIC-017 to COCRDLIC-034).
///
/// Always a 200, the empty page of `NO RECORDS FOUND FOR THIS SEARCH CONDITION.` included
/// (COCRDLIC-021).
// The schema names `CardListResponse` and `CardListRow` are fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardListResponse {
    /// The listed cards in screen order, ascending by card number, at most the page size.
    pub rows: Vec<CardListRow>,

    /// The empty rows above the first listed card: 0, except after a backward read that reached
    /// the start of the card file, whose cards fill the rows from the last one upward
    /// (COCRDLIC-029).
    #[schema(example = 0)]
    pub blank_rows_above: u32,

    /// `PAGENO`: the page number, one digit, 0 to 9 (COCRDLIC-022, COCRDLIC-030).
    #[schema(minimum = 0, maximum = 9, example = 1)]
    pub page_no: i64,

    /// The `card_ref` of the page's first key, where ENTER and F7 read from; echoed as
    /// `first_key`. `null` before any card was listed.
    pub first_key: Option<i64>,

    /// The `card_ref` of the page's last key, where F8 reads from while a next page exists: the
    /// card after the page when one exists (COCRDLIC-019). Echoed as `last_key`.
    pub last_key: Option<i64>,

    /// Whether F8 moves to a next page (COCRDLIC-019, COCRDLIC-020). Echoed as `has_next_page`.
    pub has_next_page: bool,

    /// Whether F8 has shown the last page (COCRDLIC-024, COCRDLIC-025). Echoed as
    /// `last_page_shown`.
    pub last_page_shown: bool,

    /// `ERRMSG`: the error line, such as `NO MORE RECORDS TO SHOW` or
    /// `NO PREVIOUS PAGES TO DISPLAY`, exactly as the program moves it (COCRDLIC-034); `null`
    /// when the line is empty.
    pub message: Option<String>,

    /// `INFOMSG`: `TYPE S FOR DETAIL, U TO UPDATE ANY RECORD`, or `null` (COCRDLIC-033).
    pub info_message: Option<String>,
}

impl From<rules::CardPage> for CardListResponse {
    fn from(page: rules::CardPage) -> Self {
        // TRUNCATION: checked, never lossy. The count is at most the page size, which validation
        // keeps from 1 to 100, so `u32` holds it; a count beyond `u32` saturates and never wraps.
        let blank_rows_above = u32::try_from(page.blank_rows_above()).unwrap_or(u32::MAX);
        let page_no = i64::from(page.page_no());
        let first_key = page.first_key().map(rules::CardRef::get);
        let last_key = page.last_key().map(rules::CardRef::get);
        let has_next_page = page.next_page() == rules::NextPage::Exists;
        let last_page_shown = page.last_page() == rules::LastPage::Shown;
        let message = page.message().map(str::to_owned);
        let info_message = page.info_message().map(str::to_owned);
        Self {
            rows: page
                .into_rows()
                .into_iter()
                .map(CardListRow::from)
                .collect(),
            blank_rows_above,
            page_no,
            first_key,
            last_key,
            has_next_page,
            last_page_shown,
            message,
            info_message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::{FromRequestParts, Request};
    use axum::http::StatusCode;
    use domain::copybook::{PicU, PicX};
    use domain::online::cocrdlic as rules;
    use domain::sensitive::CardNumber;
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{CARD_FILTER_MASK, CardListQuery, CardListResponse, CardListRow};
    use crate::dto::{
        FIELD_LENGTH_MESSAGE, FIELD_RANGE_MESSAGE, INVALID_QUERY, PAGE_SIZE_FIELD,
        PAGE_SIZE_MESSAGE, PageDirection, ValidQuery,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The information line of COCRDLIC-033.
    const RECORD_ACTIONS: &str = "TYPE S FOR DETAIL, U TO UPDATE ANY RECORD";

    /// A synthetic card list of `count` cards. Card `n` has handle `n`, account `n` and the card
    /// number `4000` followed by `n` in twelve digits, so handle order is card-number order.
    fn cards(count: u64) -> Result<Vec<rules::CardListRow>, Box<dyn StdError>> {
        (1..=count)
            .map(|n| {
                Ok(rules::CardListRow {
                    // TRUNCATION: checked, never lossy. `n` counts at most 15 synthetic cards,
                    // which `i64` holds; `?` refuses a larger count, never wrapping it.
                    card_ref: rules::CardRef::new(i64::try_from(n)?),
                    card_num: CardNumber::parse(&format!("4000{n:012}"))?,
                    acct_id: PicU::new(n)?,
                    status: PicX::from_text("Y")?,
                })
            })
            .collect()
    }

    /// What the service does with a validated query, over an in-memory card file: plan the
    /// read from the key and the echoed state, browse at most the plan's limit with no filter,
    /// read the look-ahead the plan asks for, and convert the page.
    fn list(cards: &[rules::CardListRow], query: &CardListQuery) -> CardListResponse {
        let key = query.direction.map(rules::Direction::from);
        let state = rules::ListState::from(query);
        let plan = if query.reread == Some(true) {
            rules::plan_reread(key, state, rules::DEFAULT_PAGE_SIZE)
        } else {
            rules::plan(key, state, rules::DEFAULT_PAGE_SIZE)
        };
        let (rows, lookahead): (Vec<rules::CardListRow>, Option<rules::CardRef>) = match plan.read()
        {
            rules::Read::Forward { start } => {
                let rows: Vec<_> = cards
                    .iter()
                    .filter(|card| start.is_none_or(|from| card.card_ref >= from))
                    .take(plan.limit())
                    .cloned()
                    .collect();
                let lookahead = plan.lookahead_after(&rows).and_then(|last| {
                    cards
                        .iter()
                        .find(|card| card.card_ref > last.card_ref)
                        .map(|card| card.card_ref)
                });
                (rows, lookahead)
            }
            rules::Read::Backward { start } => (
                cards
                    .iter()
                    .rev()
                    .filter(|card| start.is_none_or(|from| card.card_ref <= from))
                    .take(plan.limit())
                    .cloned()
                    .collect(),
                None,
            ),
        };
        CardListResponse::from(rules::page(&plan, rows, lookahead))
    }

    /// The next request of the screen: the previous response's paging state echoed unchanged,
    /// with the key pressed.
    fn echo(previous: &CardListResponse, direction: Option<PageDirection>) -> CardListQuery {
        CardListQuery {
            direction,
            page_no: Some(previous.page_no),
            first_key: previous.first_key,
            last_key: previous.last_key,
            has_next_page: Some(previous.has_next_page),
            last_page_shown: Some(previous.last_page_shown),
            ..CardListQuery::default()
        }
    }

    /// The handles of `response`'s rows.
    fn handles(response: &CardListResponse) -> Vec<i64> {
        response.rows.iter().map(|row| row.card_ref).collect()
    }

    /// `uri` through the `ValidQuery` extractor the handler uses.
    async fn extract(uri: &str) -> Result<Result<CardListQuery, ScreenError>, axum::http::Error> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<CardListQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(query)| query),
        )
    }

    /// The status, message and field of the rejection of `uri`.
    async fn rejection(
        uri: &str,
    ) -> Result<(StatusCode, String, Option<String>), Box<dyn StdError>> {
        let error = extract(uri)
            .await?
            .err()
            .ok_or_else(|| format!("{uri} was accepted"))?;
        Ok((
            error.status(),
            error.message().to_owned(),
            error.field().map(str::to_owned),
        ))
    }

    #[test]
    fn a_fresh_start_lists_seven_cards_with_the_state_to_echo() -> TestResult {
        let response = list(&cards(15)?, &CardListQuery::default());
        assert_eq!(handles(&response), (1..=7).collect::<Vec<i64>>());
        assert_eq!(
            (
                response.page_no,
                response.first_key,
                response.last_key,
                response.has_next_page,
                response.last_page_shown,
                response.blank_rows_above,
            ),
            (1, Some(1), Some(8), true, false, 0)
        );
        assert_eq!(response.message, None);
        assert_eq!(response.info_message.as_deref(), Some(RECORD_ACTIONS));
        Ok(())
    }

    #[test]
    fn the_echoed_state_pages_forward_and_backward_as_the_program_does() -> TestResult {
        let cards = cards(15)?;
        let page_1 = list(&cards, &CardListQuery::default());

        let page_2 = list(&cards, &echo(&page_1, Some(PageDirection::Forward)));
        assert_eq!(handles(&page_2), (8..=14).collect::<Vec<i64>>());
        assert_eq!((page_2.page_no, page_2.last_key), (2, Some(15)));

        // The last page: F8 marks it shown and keeps the read's message.
        let page_3 = list(&cards, &echo(&page_2, Some(PageDirection::Forward)));
        assert_eq!(handles(&page_3), vec![15]);
        assert_eq!(
            (page_3.page_no, page_3.has_next_page, page_3.last_page_shown),
            (3, false, true)
        );
        assert_eq!(page_3.message.as_deref(), Some("NO MORE RECORDS TO SHOW"));
        assert_eq!(page_3.info_message.as_deref(), Some(RECORD_ACTIONS));

        // A further F8 re-reads the page from its first key, and the echoed flag decides the text.
        let again = list(&cards, &echo(&page_3, Some(PageDirection::Forward)));
        assert_eq!((handles(&again), again.page_no), (vec![15], 3));
        assert_eq!(again.message.as_deref(), Some("NO MORE PAGES TO DISPLAY"));
        assert_eq!(again.info_message, None);

        // ENTER resets the flag, so the next F8 shows the read's message once more.
        let enter = list(&cards, &echo(&again, None));
        assert!(!enter.last_page_shown);
        let after_enter = list(&cards, &echo(&enter, Some(PageDirection::Forward)));
        assert_eq!(
            after_enter.message.as_deref(),
            Some("NO MORE RECORDS TO SHOW")
        );

        let back = list(&cards, &echo(&page_3, Some(PageDirection::Backward)));
        assert_eq!((handles(&back), back.page_no), ((8..=14).collect(), 2));
        assert!(back.has_next_page);

        // Arriving on page 1 by F7 shows the top-of-list text with the rows.
        let top = list(&cards, &echo(&back, Some(PageDirection::Backward)));
        assert_eq!((handles(&top), top.page_no), ((1..=7).collect(), 1));
        assert_eq!(top.message.as_deref(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        assert_eq!(top.info_message, None);
        Ok(())
    }

    #[test]
    fn a_short_backward_read_leaves_rows_blank_above_the_cards() -> TestResult {
        let query = CardListQuery {
            direction: Some(PageDirection::Backward),
            page_no: Some(3),
            first_key: Some(4),
            last_key: Some(11),
            has_next_page: Some(true),
            ..CardListQuery::default()
        };
        let response = list(&cards(15)?, &query);
        assert_eq!(handles(&response), vec![1, 2, 3]);
        assert_eq!(
            (
                response.blank_rows_above,
                response.page_no,
                response.first_key,
                response.last_key
            ),
            (4, 2, Some(4), Some(4))
        );
        let message = response.message.ok_or("no error line")?;
        assert!(message.starts_with("File Error: READ     on CARDDAT   returned RESP "));
        assert_eq!(message.len(), 75);
        Ok(())
    }

    #[test]
    fn the_reread_starts_from_the_first_card_and_keeps_the_page_number() -> TestResult {
        let query = CardListQuery {
            page_no: Some(2),
            first_key: Some(8),
            last_key: Some(15),
            has_next_page: Some(true),
            reread: Some(true),
            ..CardListQuery::default()
        };
        let response = list(&cards(15)?, &query);
        assert_eq!(handles(&response), (1..=7).collect::<Vec<i64>>());
        assert_eq!((response.page_no, response.first_key), (2, Some(1)));
        assert_eq!(response.message, None);
        assert_eq!(response.info_message.as_deref(), Some(RECORD_ACTIONS));
        Ok(())
    }

    #[test]
    fn page_zero_is_echoed_and_an_absent_member_is_the_fresh_state() {
        assert_eq!(
            rules::ListState::from(&CardListQuery::default()),
            rules::ListState::fresh()
        );
        let wrapped = rules::ListState::from(&CardListQuery {
            page_no: Some(0),
            ..CardListQuery::default()
        });
        assert_eq!(i64::from(wrapped.page_no), 0);
        // Validation answers 10 with a 400 first; unvalidated, it reads as absent.
        let unvalidated = rules::ListState::from(&CardListQuery {
            page_no: Some(10),
            ..CardListQuery::default()
        });
        assert_eq!(unvalidated.page_no, rules::PageNumber::FIRST);
        let echoed = rules::ListState::from(&CardListQuery {
            first_key: Some(8),
            last_key: Some(15),
            has_next_page: Some(true),
            last_page_shown: Some(true),
            ..CardListQuery::default()
        });
        assert_eq!(
            (
                echoed.first_key,
                echoed.last_key,
                echoed.next_page,
                echoed.last_page
            ),
            (
                Some(rules::CardRef::new(8)),
                Some(rules::CardRef::new(15)),
                rules::NextPage::Exists,
                rules::LastPage::Shown
            )
        );
    }

    #[test]
    fn rows_carry_the_handle_and_the_redacted_number_only() -> TestResult {
        let response = list(&cards(3)?, &CardListQuery::default());
        let body = serde_json::to_value(&response)?;
        assert_eq!(
            body["rows"][0],
            json!({
                "card_ref": 1,
                "card_num": "************0001",
                "acct_id": "00000000001",
                "status": "Y"
            })
        );
        let keys: Vec<&str> = body
            .as_object()
            .ok_or("not an object")?
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "blank_rows_above",
                "first_key",
                "has_next_page",
                "info_message",
                "last_key",
                "last_page_shown",
                "message",
                "page_no",
                "rows"
            ]
        );
        let text = body.to_string();
        assert!(!text.contains("4000000000000001") && !text.contains("40000000"));
        assert!(!format!("{response:?}").contains("40000000"));
        // COCRDLIC-032: a stored space travels as the space it is.
        let blank = CardListRow::from(rules::CardListRow {
            status: PicX::spaces(),
            ..cards(1)?.remove(0)
        });
        assert_eq!(blank.status, " ");
        assert_eq!(serde_json::to_value(&blank)?["status"], json!(" "));
        Ok(())
    }

    #[test]
    fn the_card_filter_is_never_serialised_or_formatted() -> TestResult {
        let query = CardListQuery {
            acct_id: Some("00000000050".to_owned()),
            card_num: Some("4000000000000001".to_owned()),
            ..CardListQuery::default()
        };
        let body = serde_json::to_value(&query)?;
        assert!(body.get("card_num").is_none());
        assert_eq!(body["acct_id"], "00000000050");
        let debug = format!("{query:?}");
        assert!(debug.contains(CARD_FILTER_MASK), "{debug}");
        assert!(!debug.contains("4000000000000001"));
        assert!(format!("{:?}", CardListQuery::default()).contains("card_num: None"));
        Ok(())
    }

    #[tokio::test]
    async fn the_query_string_carries_the_filters_the_key_and_the_echo() -> TestResult {
        let query = extract(
            "/api/v1/cards?acct_id=00000000050&card_num=0000000000000000&direction=forward\
             &page_no=0&first_key=1&last_key=8&has_next_page=true&last_page_shown=false\
             &reread=false&page_size=100",
        )
        .await?
        .map_err(|error| error.to_string())?;
        assert_eq!(
            query,
            CardListQuery {
                acct_id: Some("00000000050".to_owned()),
                card_num: Some("0000000000000000".to_owned()),
                direction: Some(PageDirection::Forward),
                page_no: Some(0),
                first_key: Some(1),
                last_key: Some(8),
                has_next_page: Some(true),
                last_page_shown: Some(false),
                reread: Some(false),
                page_size: Some(100),
            }
        );
        let empty = extract("/api/v1/cards")
            .await?
            .map_err(|error| error.to_string())?;
        assert_eq!(empty, CardListQuery::default());
        Ok(())
    }

    #[tokio::test]
    async fn the_query_string_is_validated_before_any_read() -> TestResult {
        let page_size = (
            StatusCode::UNPROCESSABLE_ENTITY,
            PAGE_SIZE_MESSAGE.to_owned(),
            Some(PAGE_SIZE_FIELD.to_owned()),
        );
        for uri in [
            "/api/v1/cards?page_size=101",
            "/api/v1/cards?page_size=0",
            "/api/v1/cards?page_size=101&page_no=10&acct_id=000000000500",
        ] {
            assert_eq!(rejection(uri).await?, page_size, "{uri}");
        }
        let input = |message: &str, field: &str| {
            (
                StatusCode::BAD_REQUEST,
                message.to_owned(),
                Some(field.to_owned()),
            )
        };
        for (uri, expected) in [
            (
                "/api/v1/cards?page_no=10",
                input(FIELD_RANGE_MESSAGE, "page_no"),
            ),
            (
                "/api/v1/cards?page_no=-1",
                input(FIELD_RANGE_MESSAGE, "page_no"),
            ),
            (
                "/api/v1/cards?acct_id=000000000500",
                input(FIELD_LENGTH_MESSAGE, "acct_id"),
            ),
            (
                "/api/v1/cards?card_num=40000000000000010",
                input(FIELD_LENGTH_MESSAGE, "card_num"),
            ),
        ] {
            assert_eq!(rejection(uri).await?, expected, "{uri}");
        }
        for uri in [
            "/api/v1/cards?direction=sideways",
            "/api/v1/cards?first_key=card",
            "/api/v1/cards?has_next_page=Y",
        ] {
            assert_eq!(
                rejection(uri).await?,
                (StatusCode::BAD_REQUEST, INVALID_QUERY.to_owned(), None),
                "{uri}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_schemas_carry_the_contract() -> TestResult {
        assert_eq!(CardListQuery::name(), "CardListQuery");
        assert_eq!(CardListRow::name(), "CardListRow");
        assert_eq!(CardListResponse::name(), "CardListResponse");

        let params: Vec<Value> = CardListQuery::into_params(|| None)
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        let names: Vec<&str> = params
            .iter()
            .filter_map(|param| param["name"].as_str())
            .collect();
        assert_eq!(
            names,
            [
                "acct_id",
                "card_num",
                "direction",
                "page_no",
                "first_key",
                "last_key",
                "has_next_page",
                "last_page_shown",
                "reread",
                "page_size"
            ]
        );
        assert!(params.iter().all(|param| param["in"] == "query"));
        assert!(params.iter().all(|param| param["required"] == json!(false)));
        let param = |name: &str| params.iter().find(|param| param["name"] == name);
        let card = param("card_num").ok_or("no card_num parameter")?;
        assert_eq!(card["example"], "************5740");
        let size = param("page_size").ok_or("no page_size parameter")?;
        assert_eq!(
            (&size["schema"]["minimum"], &size["schema"]["maximum"]),
            (&json!(1), &json!(100))
        );
        let page_no = param("page_no").ok_or("no page_no parameter")?;
        assert_eq!(
            (&page_no["schema"]["minimum"], &page_no["schema"]["maximum"]),
            (&json!(0), &json!(9))
        );

        let query = serde_json::to_value(CardListQuery::schema())?;
        assert_eq!(query["properties"]["card_num"]["writeOnly"], json!(true));
        assert_eq!(
            query["properties"]["card_num"]["example"],
            "************5740"
        );
        let row = serde_json::to_value(CardListRow::schema())?;
        assert_eq!(row["properties"]["card_num"]["type"], "string");
        assert_eq!(row["properties"]["card_num"]["example"], "************5740");
        Ok(())
    }
}
