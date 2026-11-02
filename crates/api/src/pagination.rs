//! Shared pagination of the list endpoints (AAP 0.6.4): the query every list
//! takes and the page every list returns.
//!
//! The COBOL list programs page through a keyed file with a browse, PF8 for the
//! next screen and PF7 for the previous one:
//!
//! | Endpoint | Program | Browse | Rows per screen |
//! |---|---|---|---|
//! | `GET /transactions` | `COTRN00C` | CICS `STARTBR`, `READNEXT`, `READPREV` on `TRANSACT` | 10 (P-COTRN00C-006) |
//! | `GET /admin/users` | `COUSR00C` | CICS `STARTBR`, `READNEXT`, `READPREV` on `USRSEC` | 10 (P-COUSR00C-003) |
//! | `GET /cards` | `COCRDLIC` | CICS `STARTBR`, `READNEXT`, `READPREV` on `CARDDAT` | 7 (P-COCRDLIC-003) |
//! | `GET /admin/transaction-types` | `COTRTLIC` | Db2 cursors `C-TR-TYPE-FORWARD`, `C-TR-TYPE-BACKWARD` | 7 (P-COTRTLIC-003) |
//! | `GET /authorizations` | `COPAUS0C` | DL/I `GNP` under the account summary | 5 (P-COPAUS0C-010) |
//! | `GET /rewards/history/{account_id}` | none (rewards) | `accrual_id`, newest first | 10 |
//!
//! The browse paragraphs:
//!
//! - `COTRN00C`: `PROCESS-PAGE-FORWARD` and `PROCESS-PAGE-BACKWARD` over
//!   `STARTBR-TRANSACT-FILE`, `READNEXT-TRANSACT-FILE` and
//!   `READPREV-TRANSACT-FILE` [app/cbl/COTRN00C.cbl:L279]
//!   [app/cbl/COTRN00C.cbl:L333] [app/cbl/COTRN00C.cbl:L591]
//!   [app/cbl/COTRN00C.cbl:L624] [app/cbl/COTRN00C.cbl:L658].
//! - `COUSR00C`: `PROCESS-PAGE-FORWARD` and `PROCESS-PAGE-BACKWARD` over
//!   `STARTBR-USER-SEC-FILE`, `READNEXT-USER-SEC-FILE` and
//!   `READPREV-USER-SEC-FILE` [app/cbl/COUSR00C.cbl:L282]
//!   [app/cbl/COUSR00C.cbl:L336] [app/cbl/COUSR00C.cbl:L586]
//!   [app/cbl/COUSR00C.cbl:L619] [app/cbl/COUSR00C.cbl:L653].
//! - `COCRDLIC`: `9000-READ-FORWARD` (`STARTBR`, `READNEXT`) and
//!   `9100-READ-BACKWARDS` (`STARTBR`, `READPREV`), `WS-MAX-SCREEN-LINES` 7
//!   [app/cbl/COCRDLIC.cbl:L1123] [app/cbl/COCRDLIC.cbl:L1264]
//!   [app/cbl/COCRDLIC.cbl:L177-178].
//! - `COTRTLIC`: `8000-READ-FORWARD` and `8100-READ-BACKWARDS`,
//!   `WS-MAX-SCREEN-LINES` 7
//!   [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1603]
//!   [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1727]
//!   [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L60].
//! - `COPAUS0C`: `PROCESS-PAGE-FORWARD` and `GET-AUTHORIZATIONS`
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L415]
//!   [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L458].
//!
//! # Query
//!
//! [`PageQuery`] carries the three parameters every list shares:
//!
//! - `start_key`: the key the browse starts at, the `RIDFLD` of `STARTBR` or the
//!   start value of the cursor. It stays raw text, so the rule's own key edit
//!   answers a malformed key with the audited COBOL message (for example
//!   `COTRN00C`'s `Tran ID must be Numeric ...`), never the extractor.
//! - `direction`: [`PageDirection::Forward`] for PF8 and ENTER,
//!   [`PageDirection::Backward`] for PF7. Forward when absent.
//! - `page_size`: 1 to [`MAX_PAGE_SIZE`] rows; the program's screen rows when
//!   absent. A value outside the range fails to deserialize, so the query
//!   extractor [`ApiQuery`](crate::error::ApiQuery) answers it 400 with code
//!   `INVALID` and the rejection text.
//!
//! [`PageQuery::resolve`] turns the query into the start key, the domain
//! [`Direction`] and the row count a rule's browse input takes; the rule builds
//! its typed [`carddemo_domain::ports::Browse`] from them after its key edit.
//!
//! # Response
//!
//! [`Page`] is `{items, next_key, prev_key, message?}`:
//!
//! - `items` are the rows in the COBOL key order. The browse order is the only
//!   order: no sort parameter exists.
//! - An empty page is a 200 with `items: []`, also where the COBOL ends the task
//!   on an empty browse (E-27, E-71).
//! - `message` is the page-boundary text the COBOL shows without setting its
//!   error flag, such as `You are already at the top of the page...` (E-54),
//!   copied unchanged from the rule.

use std::fmt;

use carddemo_domain::ports::Direction;
use serde::de::{self, Deserializer, Unexpected, Visitor};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// The most rows one page may hold, whatever the program's screen holds.
pub const MAX_PAGE_SIZE: u16 = 100;

/// The way a page request reads from `start_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PageDirection {
    /// PF8 (and ENTER): ascending key order, CICS `READNEXT` after `STARTBR`
    /// (`COTRN00C`, `COUSR00C`, `COCRDLIC`), the forward Db2 cursor
    /// (`COTRTLIC`) or the next `GNP` (`COPAUS0C`).
    #[default]
    Forward,
    /// PF7: descending key order, CICS `READPREV` after `STARTBR`
    /// (`COTRN00C`, `COUSR00C`, `COCRDLIC`) or the backward Db2 cursor
    /// (`COTRTLIC`).
    Backward,
}

impl From<PageDirection> for Direction {
    fn from(direction: PageDirection) -> Self {
        match direction {
            PageDirection::Forward => Self::Forward,
            PageDirection::Backward => Self::Backward,
        }
    }
}

/// A requested page size: 1 to [`MAX_PAGE_SIZE`] rows.
///
/// The range is checked as the query deserializes, so an out-of-range
/// `page_size` is a query rejection that
/// [`ApiQuery`](crate::error::ApiQuery) answers 400 with code `INVALID`. This
/// crate therefore holds no message text of its own for it, and
/// [`PageQuery::resolve`] cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSize(u16);

impl PageSize {
    /// The page size of `rows`, or `None` when `rows` is 0 or above
    /// [`MAX_PAGE_SIZE`].
    #[must_use]
    pub const fn new(rows: u16) -> Option<Self> {
        if rows == 0 || rows > MAX_PAGE_SIZE {
            None
        } else {
            Some(Self(rows))
        }
    }

    /// The number of rows.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PageSize {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // `deserialize_any` rather than `deserialize_u16`: a query struct that
        // embeds `PageQuery` with `#[serde(flatten)]` buffers every query value
        // as text, which `deserialize_u16` refuses. The visitor parses text
        // itself and also takes the integers a JSON document carries.
        deserializer.deserialize_any(PageSizeVisitor)
    }
}

/// Reads a [`PageSize`] from an integer or from the decimal text of one.
struct PageSizeVisitor;

impl Visitor<'_> for PageSizeVisitor {
    type Value = PageSize;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "an integer from 1 to {MAX_PAGE_SIZE}")
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<PageSize, E> {
        u16::try_from(value)
            .ok()
            .and_then(PageSize::new)
            .ok_or_else(|| E::invalid_value(Unexpected::Unsigned(value), &self))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<PageSize, E> {
        match u64::try_from(value) {
            Ok(unsigned) => self.visit_u64(unsigned),
            Err(_) => Err(E::invalid_value(Unexpected::Signed(value), &self)),
        }
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<PageSize, E> {
        // The text is parsed as `str::parse` parses an integer, the rule axum's
        // own query deserializer applies to integer fields.
        match value.parse::<u64>() {
            Ok(number) => self.visit_u64(number),
            Err(_) => Err(E::invalid_value(Unexpected::Str(value), &self)),
        }
    }
}

/// The pagination query of every list endpoint: `start_key`, `direction` and
/// `page_size`.
///
/// Handlers extract it with [`ApiQuery<PageQuery>`](crate::error::ApiQuery)
/// and declare it with `params(PageQuery)` in `#[utoipa::path]`. A list with
/// filters of its own extracts them as a second query struct, since unknown
/// parameters are ignored here. A list whose `start_key` has a grammar to
/// publish declares the three parameters in a query struct of its own and
/// converts it into this one, so [`PageQuery::resolve`] applies the same
/// defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    /// The key the page starts at, as the list shows it. Absent for the first
    /// page forward; what an absent key reads backward is the list's own, as
    /// its operation states.
    pub start_key: Option<String>,
    /// `forward` (next page, the default) or `backward` (previous page).
    // Inlined: query parameters register no component schema, so a reference
    // to `PageDirection` would dangle in the OpenAPI document.
    #[param(inline)]
    pub direction: Option<PageDirection>,
    /// Rows on the page, from 1 to 100. Defaults to the rows of the list's
    /// screen.
    #[param(value_type = Option<u16>, minimum = 1, maximum = 100)]
    pub page_size: Option<PageSize>,
}

impl PageQuery {
    /// The start key, the browse direction and the row count of the request.
    ///
    /// `direction` defaults to [`Direction::Forward`] and `page_size` to
    /// `default_rows`, the rule's screen-rows constant (for example
    /// [`carddemo_domain::rules::transaction_list::SCREEN_ROWS`]). The start key
    /// is returned as entered, for the rule's key edit.
    ///
    /// This never fails: `page_size` was range-checked when the query
    /// deserialized, so handlers call it without `?`.
    #[must_use]
    pub fn resolve(self, default_rows: u16) -> (Option<String>, Direction, u16) {
        (
            self.start_key,
            self.direction.unwrap_or_default().into(),
            self.page_size.map_or(default_rows, PageSize::get),
        )
    }
}

/// One page of a list: `{items, next_key, prev_key, message?}`.
///
/// The keys are the start keys of the neighbouring pages, as the rule's page
/// gives them: zero-padded or space-trimmed key text that a following request
/// passes back unchanged as `start_key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Page<T> {
    /// The rows of the page, in the list's key order. Empty when the page
    /// shows none.
    pub items: Vec<T>,
    /// The `start_key` of the next page, read with `direction=forward`; `null`
    /// when no page follows.
    // Required although nullable: the key is always serialized, as `null`
    // when absent, so the published schema says so.
    #[schema(required = true)]
    pub next_key: Option<String>,
    /// The `start_key` of the previous page, read with `direction=backward`;
    /// `null` when no page precedes it.
    #[schema(required = true)]
    pub prev_key: Option<String>,
    /// The page-boundary text the COBOL program shows with this page, such as
    /// `You are already at the top of the page...`. Omitted when it shows none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl<T> Page<T> {
    /// A page of `items` with its neighbouring keys and the rule's message,
    /// each copied unchanged.
    #[must_use]
    pub fn new(
        items: Vec<T>,
        next_key: Option<String>,
        prev_key: Option<String>,
        message: Option<String>,
    ) -> Self {
        Self {
            items,
            next_key,
            prev_key,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::{FromRequestParts, Query};
    use axum::http::{Request, StatusCode, Uri};
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::ports::Direction;
    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{MAX_PAGE_SIZE, Page, PageDirection, PageQuery, PageSize};
    use crate::error::ApiQuery;

    /// The expectation every page-size rejection names.
    const EXPECTED: &str = "expected an integer from 1 to 100";

    fn query(text: &str) -> Result<PageQuery, String> {
        let uri: Uri = format!("/list?{text}").parse().expect("a valid URI");
        Query::<PageQuery>::try_from_uri(&uri)
            .map(|Query(page)| page)
            .map_err(|rejection| rejection.body_text())
    }

    /// A row type for the generic page.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
    struct Row {
        id: String,
    }

    #[test]
    fn page_size_accepts_one_to_max_and_rejects_the_rest() {
        assert_eq!(MAX_PAGE_SIZE, 100);
        // (query text, accepted rows)
        let cases = [
            ("page_size=1", Some(1)),
            ("page_size=7", Some(7)),
            ("page_size=100", Some(100)),
            ("page_size=0", None),
            ("page_size=101", None),
            ("page_size=65536", None),
            ("page_size=-1", None),
            ("page_size=abc", None),
            ("page_size=", None),
            ("page_size=5.0", None),
        ];
        for (text, rows) in cases {
            match (query(text), rows) {
                (Ok(page), Some(rows)) => {
                    assert_eq!(page.page_size.map(PageSize::get), Some(rows), "{text}");
                }
                (Err(rejection), None) => {
                    assert!(rejection.contains(EXPECTED), "{text}: {rejection}");
                }
                (outcome, _) => panic!("{text}: unexpected {outcome:?}"),
            }
        }
        assert_eq!(PageSize::new(0), None);
        assert_eq!(PageSize::new(MAX_PAGE_SIZE + 1), None);
        assert_eq!(PageSize::new(MAX_PAGE_SIZE).map(PageSize::get), Some(100));
    }

    #[test]
    fn page_size_reads_json_integers_and_refuses_other_json() {
        // (JSON value, accepted rows)
        let cases = [
            (json!(5), Some(5)),
            (json!("10"), Some(10)),
            (json!(0), None),
            (json!(101), None),
            (json!(-3), None),
            (json!(1.5), None),
            (json!(null), None),
            (json!(true), None),
        ];
        for (value, rows) in cases {
            let outcome = serde_json::from_value::<PageSize>(value.clone());
            assert_eq!(
                outcome.as_ref().ok().map(|size| size.get()),
                rows,
                "{value}"
            );
            if let Err(error) = outcome {
                assert!(error.to_string().contains(EXPECTED), "{value}: {error}");
            }
        }
    }

    #[test]
    fn direction_reads_forward_and_backward_only() {
        // (query text, parsed direction)
        let cases = [
            ("direction=forward", Some(PageDirection::Forward)),
            ("direction=backward", Some(PageDirection::Backward)),
            ("direction=Backward", None),
            ("direction=up", None),
            ("direction=", None),
        ];
        for (text, direction) in cases {
            assert_eq!(
                query(text).ok().map(|page| page.direction),
                direction.map(Some),
                "{text}"
            );
        }
        assert_eq!(Direction::from(PageDirection::Forward), Direction::Forward);
        assert_eq!(
            Direction::from(PageDirection::Backward),
            Direction::Backward
        );
        assert_eq!(PageDirection::default(), PageDirection::Forward);
        assert_eq!(
            serde_json::to_value(PageDirection::Backward).expect("serializes"),
            json!("backward")
        );
    }

    #[test]
    fn resolve_applies_the_defaults_and_keeps_the_start_key_as_entered() {
        // (query text, default rows, resolved start key, direction, rows)
        let cases = [
            ("", 10, None, Direction::Forward, 10),
            ("", 7, None, Direction::Forward, 7),
            ("direction=backward", 5, None, Direction::Backward, 5),
            ("page_size=3", 10, None, Direction::Forward, 3),
            (
                "start_key=0000000000000042",
                10,
                Some("0000000000000042"),
                Direction::Forward,
                10,
            ),
            (
                "start_key=ab%20c&direction=backward&page_size=100",
                7,
                Some("ab c"),
                Direction::Backward,
                100,
            ),
            ("start_key=", 10, Some(""), Direction::Forward, 10),
            ("other=1&page_size=2", 10, None, Direction::Forward, 2),
        ];
        for (text, default_rows, start_key, direction, rows) in cases {
            let resolved = query(text).expect("a valid query").resolve(default_rows);
            assert_eq!(
                resolved,
                (start_key.map(str::to_owned), direction, rows),
                "{text}"
            );
        }
        assert_eq!(
            PageQuery::default().resolve(10),
            (None, Direction::Forward, 10)
        );
    }

    #[test]
    fn a_flattened_page_query_reads_its_parameters_from_text() {
        #[derive(Debug, Deserialize)]
        struct Filtered {
            account_id: Option<String>,
            #[serde(flatten)]
            page: PageQuery,
        }

        let uri: Uri = "/list?account_id=00000000001&page_size=5&direction=backward"
            .parse()
            .expect("a valid URI");
        let Query(filtered) = Query::<Filtered>::try_from_uri(&uri).expect("deserializes");
        assert_eq!(filtered.account_id.as_deref(), Some("00000000001"));
        assert_eq!(filtered.page.resolve(10), (None, Direction::Backward, 5));

        let uri: Uri = "/list?page_size=0".parse().expect("a valid URI");
        let rejection = Query::<Filtered>::try_from_uri(&uri).expect_err("out of range");
        assert!(
            rejection.body_text().contains(EXPECTED),
            "{}",
            rejection.body_text()
        );
    }

    #[tokio::test]
    async fn api_query_answers_an_out_of_range_page_size_with_400_invalid() {
        for text in ["page_size=0", "page_size=101", "direction=sideways"] {
            let (mut parts, ()) = Request::builder()
                .uri(format!("/cards?{text}"))
                .body(())
                .expect("a valid request")
                .into_parts();
            let error = ApiQuery::<PageQuery>::from_request_parts(&mut parts, &())
                .await
                .expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{text}");
            assert_eq!(
                error.body().message,
                query(text).expect_err("rejected"),
                "{text}"
            );
            assert!(error.body().fields.is_empty(), "{text}");
        }
    }

    #[test]
    fn page_json_keeps_null_keys_and_omits_an_absent_message() {
        let first: Page<Row> = Page::new(
            vec![Row {
                id: "USER0001".to_owned(),
            }],
            Some("USER0001".to_owned()),
            None,
            None,
        );
        let first_json = json!({
            "items": [{"id": "USER0001"}],
            "next_key": "USER0001",
            "prev_key": null,
        });
        assert_eq!(
            serde_json::to_value(&first).expect("serializes"),
            first_json
        );
        assert_eq!(
            serde_json::from_value::<Page<Row>>(first_json).expect("deserializes"),
            first
        );

        let top: Page<Row> = Page::new(
            Vec::new(),
            None,
            None,
            Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE.to_owned()),
        );
        assert_eq!(
            serde_json::to_value(&top).expect("serializes"),
            json!({
                "items": [],
                "next_key": null,
                "prev_key": null,
                "message": "You are already at the top of the page...",
            })
        );
    }

    #[test]
    fn openapi_params_are_optional_query_parameters_with_the_page_size_range() {
        let params = serde_json::to_value(PageQuery::into_params(|| None)).expect("serializes");
        let names: Vec<&str> = params
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["start_key", "direction", "page_size"]);
        for param in params.as_array().expect("an array") {
            assert_eq!(param["in"], json!("query"), "{param}");
            assert_eq!(param["required"], json!(false), "{param}");
        }
        assert_eq!(params[0]["schema"]["type"], json!("string"));
        assert_eq!(params[1]["schema"]["type"], json!("string"));
        assert_eq!(params[1]["schema"]["enum"], json!(["forward", "backward"]));
        assert_eq!(params[2]["schema"]["type"], json!("integer"));
        assert_eq!(params[2]["schema"]["minimum"], json!(1));
        assert_eq!(params[2]["schema"]["maximum"], json!(100));
        let text = params.to_string();
        assert!(!text.contains("$ref"), "{text}");
    }

    #[test]
    fn page_schema_requires_items_and_both_keys_only() {
        let schema = serde_json::to_value(Page::<Row>::schema()).expect("serializes");
        assert_eq!(schema["required"], json!(["items", "next_key", "prev_key"]));
        assert_eq!(schema["properties"]["items"]["type"], json!("array"));
        assert!(
            schema["properties"]["items"]["items"].is_object(),
            "{schema}"
        );
        for key in ["next_key", "prev_key", "message"] {
            assert_eq!(
                schema["properties"][key]["type"],
                json!(["string", "null"]),
                "{key}"
            );
        }
        let direction: Value = serde_json::to_value(PageDirection::schema()).expect("serializes");
        assert_eq!(direction["enum"], json!(["forward", "backward"]));
    }
}
