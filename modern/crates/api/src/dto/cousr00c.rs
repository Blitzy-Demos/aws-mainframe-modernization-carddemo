//! The contract of endpoint 16, `GET /api/v1/users` \[A\]: the user list of transaction `CU00`,
//! program COUSR00C, map `COUSR0A` (AAP 0.3.4.1).
//!
//! ENTER lists from the search field and F8 and F7 page forward and backward. The `U` and `D`
//! row selections and F3 only navigate and send no request (COUSR00C-004, COUSR00C-007 to
//! COUSR00C-011). The endpoint is a Bounded Pagination collection: ten rows by default
//! ([`rules::DEFAULT_PAGE_SIZE`]), `page_size` 1 to 100, 422 outside.
//!
//! # Paging state
//!
//! COUSR00C keeps its paging state in the COMMAREA between interactions: the first and last user
//! IDs, the page number and the next-page indicator (COUSR00C-006). Every [`UserListResponse`]
//! returns them as `first_key`, `last_key`, `page_no` and `has_next_page`, and the next request
//! echoes them unchanged in the [`UserListQuery`] members of the same names. ENTER echoes the
//! keys too: a page that leaves row 1 or row 10 blank keeps the key the request carried
//! (COUSR00C-016). `page_key` is the key an F7 or F8 reads from, the first user ID backward and
//! the last forward; when present it takes the place of the echoed key of its direction. The
//! state and the search value are request data: they position the list and grant nothing (AAP
//! 0.5.1). The admin gate is the handler's `AdminUser` extractor (COUSR00C-001).
//!
//! # Rows
//!
//! A page is `page_size` row positions, row 1 first, `null` where a row is blank: a forward page
//! fills from row 1 and a backward page from the last row up (COUSR00C-013, COUSR00C-018,
//! COUSR00C-024). `rows` is `null` when nothing was read and the screen keeps the rows it shows
//! (COUSR00C-020, COUSR00C-021). A row carries the user ID, names and type, never a password or
//! its hash (COUSR00C-028, Cardholder Data Redaction).

use domain::copybook::{PicU, PicX};
use domain::online::cousr00c as rules;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{PageDirection, resolve_page_size, text};

/// The query string of endpoint 16: the search field, the paging key and direction, the paging
/// state the last response returned, and the page size.
///
/// No member is required: a request without any is the first entry of the screen, ENTER with a
/// blank search field (COUSR00C-003, COUSR00C-012).
// The schema name `UserListQuery` is the frontend's `components["schemas"]["UserListQuery"]`
// (Generated API Types Only). The state members beyond `page_key` and `page_no` carry the rest
// of `CDEMO-CU00-INFO`, which COUSR00C-006 and COUSR00C-016 require a request to echo.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct UserListQuery {
    /// `USRIDIN`, the search field: ENTER lists from the first user ID at or above it, as typed,
    /// with no edit; absent or blank, from the first user (COUSR00C-012). F7 and F8 ignore it
    /// (COUSR00C-017). At most the map's 8 characters (COUSR00-015).
    #[validate(length(max = 8))]
    #[schema(max_length = 8)]
    #[param(max_length = 8)]
    pub start_user_id: Option<String>,
    /// The user ID F7 or F8 reads from: the first user ID shown for `backward`, the last for
    /// `forward` (COUSR00C-017, COUSR00C-022). Takes the place of `first_key` or `last_key`
    /// respectively; ignored without a `direction`.
    #[validate(length(max = 8))]
    #[schema(max_length = 8)]
    #[param(max_length = 8)]
    pub page_key: Option<String>,
    /// `forward` for F8, `backward` for F7; absent for ENTER and the first entry.
    pub direction: Option<PageDirection>,
    /// `first_key` of the last response, echoed: `CDEMO-CU00-USRID-FIRST` (COUSR00C-006).
    #[validate(length(max = 8))]
    #[schema(max_length = 8)]
    #[param(max_length = 8)]
    pub first_key: Option<String>,
    /// `last_key` of the last response, echoed: `CDEMO-CU00-USRID-LAST` (COUSR00C-006).
    #[validate(length(max = 8))]
    #[schema(max_length = 8)]
    #[param(max_length = 8)]
    pub last_key: Option<String>,
    /// `page_no` of the last response, echoed: `CDEMO-CU00-PAGE-NUM PIC 9(08)`, so 0 to
    /// 99999999 (COUSR00C-006). Absent, page 0, the page number of a first entry. F7 reads only
    /// above page 1 (COUSR00C-021, COUSR00C-022).
    // Page 0 is admitted because a response returns it: ENTER that reads no row keeps the 0 it
    // starts from (COUSR00C-012, COUSR00C-025), and 99999999 + 1 stores 0 (COUSR00C-014).
    #[validate(range(min = 0, max = 99_999_999))]
    #[schema(minimum = 0, maximum = 99_999_999)]
    #[param(minimum = 0, maximum = 99_999_999)]
    pub page_no: Option<i64>,
    /// `has_next_page` of the last response, echoed: `CDEMO-CU00-NEXT-PAGE-FLG`. F8 without it,
    /// or with `false`, answers `You are already at the bottom of the page...` and reads nothing
    /// (COUSR00C-020).
    pub has_next_page: Option<bool>,
    /// Rows per page, 1 to 100; absent, the map's 10 (Bounded Pagination). Outside the range,
    /// 422. The screen never sends it.
    #[validate(range(min = 1, max = 100))]
    #[schema(minimum = 1, maximum = 100)]
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

impl UserListQuery {
    /// The interaction the request makes: F8 or F7 for a `direction`, otherwise ENTER from the
    /// read key [`rules::start_user_id`] makes of the search field (COUSR00C-012,
    /// COUSR00C-017, COUSR00C-022).
    #[must_use]
    pub fn action(&self) -> rules::Action {
        match self.direction {
            Some(direction) => rules::Action::Page(direction.into()),
            None => rules::Action::Enter {
                start: rules::start_user_id(self.start_user_id.as_deref().unwrap_or_default()),
            },
        }
    }

    /// The paging state the request carries back (COUSR00C-006): the echoed keys, with
    /// `page_key` in the place of the key of its direction, the page number, and the next-page
    /// indicator, `Y` only for `has_next_page` `true`.
    ///
    /// A key is moved into `PIC X(08)` as text: left-justified and space-padded, blank when
    /// absent. A key the response returned therefore comes back as the same 8 bytes.
    #[must_use]
    pub fn paging_state(&self) -> rules::PagingState {
        let page_key = self.page_key.as_deref();
        let first_key = self.first_key.as_deref();
        let last_key = self.last_key.as_deref();
        let (first_key, last_key) = match self.direction {
            Some(PageDirection::Backward) => (page_key.or(first_key), last_key),
            Some(PageDirection::Forward) => (first_key, page_key.or(last_key)),
            None => (first_key, last_key),
        };
        rules::PagingState {
            first_key: key_field(first_key),
            last_key: key_field(last_key),
            page_no: page_number(self.page_no),
            next_page: match self.has_next_page {
                Some(true) => rules::NextPage::Yes,
                Some(false) | None => rules::NextPage::No,
            },
        }
    }

    /// The rows a page shows: `page_size`, or [`rules::DEFAULT_PAGE_SIZE`] when it is absent
    /// (Bounded Pagination, COUSR00C-013).
    #[must_use]
    pub fn resolved_page_size(&self) -> usize {
        // TRUNCATION: checked both ways, never lossy. The default 10 and every validated size
        // (1 to 100) fit `u32` and `usize`; a conversion either refuses gives the default.
        u32::try_from(rules::DEFAULT_PAGE_SIZE)
            .ok()
            .map(|default| resolve_page_size(self.page_size, default))
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(rules::DEFAULT_PAGE_SIZE)
    }
}

/// A key member moved into `PIC X(08)`: its first 8 bytes, space-padded; spaces when absent.
fn key_field(key: Option<&str>) -> PicX<8> {
    key.map_or_else(PicX::spaces, |key| PicX::moved_from(key.as_bytes()))
}

/// The echoed page number as `PIC 9(08)`; page 0 when absent.
fn page_number(page_no: Option<i64>) -> PicU<8> {
    // TRUNCATION: checked, never lossy. Validation admits 0 to 99999999, which `u64` and
    // PIC 9(08) hold exactly; a value either conversion refuses is page 0, as when absent.
    page_no
        .and_then(|page_no| u64::try_from(page_no).ok())
        .and_then(|page_no| PicU::new(page_no).ok())
        .unwrap_or(PicU::ZERO)
}

/// One row of the list: the four `SEC-USER-DATA` fields `POPULATE-USER-DATA` moves to the map,
/// trailing spaces removed (COUSR00C-028).
///
/// There is no password member: `SEC-USR-PWD` never reaches the map, and the list never reads
/// the stored hash (CSUSR01Y-004, Cardholder Data Redaction).
// The schema name `UserListRow` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UserListRow {
    /// `USRIDnn` (8), `SEC-USR-ID`: the user ID a `U` or `D` selection carries to the update or
    /// delete screen (COUSR00C-008, COUSR00C-009).
    #[schema(max_length = 8)]
    pub user_id: String,
    /// `FNAMEnn` (20), `SEC-USR-FNAME`.
    #[schema(max_length = 20)]
    pub first_name: String,
    /// `LNAMEnn` (20), `SEC-USR-LNAME`.
    #[schema(max_length = 20)]
    pub last_name: String,
    /// `UTYPEnn` (1), `SEC-USR-TYPE`: `A` for an administrator, `U` for a regular user, as
    /// stored.
    #[schema(max_length = 1)]
    pub user_type: String,
}

impl From<rules::UserListRow> for UserListRow {
    fn from(row: rules::UserListRow) -> Self {
        Self {
            user_id: text(&row.user_id),
            first_name: text(&row.first_name),
            last_name: text(&row.last_name),
            user_type: text(&row.user_type),
        }
    }
}

/// The 200 of endpoint 16, a page or a message with the paging state (COUSR00C-006,
/// COUSR00C-013 to COUSR00C-025).
///
/// The `ScreenError`s are the handler's: 401 and 403 from the gate, 400 for a member that
/// breaks its rule, 422 for `page_size`, and 500 `Unable to lookup User...` (COUSR00C-026).
// The schema name `UserListResponse` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UserListResponse {
    /// The page: `page_size` row positions, row 1 first, `null` for a blank row (COUSR00C-013,
    /// COUSR00C-018, COUSR00C-024). `null` when nothing was read: the screen keeps the rows, page
    /// number and search field it shows (COUSR00C-020, COUSR00C-021).
    // Row positions rather than a list of filled rows: a list could not tell ten blank rows
    // (COUSR00C-018) from kept rows (COUSR00C-020), nor place a short backward page at the
    // bottom of the screen (COUSR00C-024).
    pub rows: Option<Vec<Option<UserListRow>>>,
    /// `CDEMO-CU00-PAGE-NUM PIC 9(08)`; the screen shows it as eight digits with leading zeros
    /// (COUSR00C-029). Echo it as the next request's `page_no`.
    #[schema(maximum = 99_999_999)]
    pub page_no: u64,
    /// `CDEMO-CU00-USRID-FIRST`, the key of F7: the user ID of row 1 when this page filled it,
    /// otherwise the key the request carried; `null` when blank (COUSR00C-016). Echo it as the
    /// next request's `first_key`, and as its `page_key` for `backward`.
    #[schema(max_length = 8)]
    pub first_key: Option<String>,
    /// `CDEMO-CU00-USRID-LAST`, the key of F8: the user ID of the last row when this page filled
    /// it, otherwise the key the request carried; `null` when blank (COUSR00C-016). Echo it as
    /// the next request's `last_key`, and as its `page_key` for `forward`.
    #[schema(max_length = 8)]
    pub last_key: Option<String>,
    /// `CDEMO-CU00-NEXT-PAGE-FLG` = `Y`: whether F8 reads (COUSR00C-014, COUSR00C-020). F7 sets
    /// it whether or not a user follows (COUSR00C-021). Echo it as the next request's
    /// `has_next_page`.
    pub has_next_page: bool,
    /// The message row: `You have reached the bottom of the page...`, `You have reached the top
    /// of the page...`, `You are already at the bottom of the page...`, `You are already at the
    /// top of the page...` or `You are at the top of the page...`; `null` for none
    /// (COUSR00C-014 to COUSR00C-025).
    pub message: Option<String>,
}

impl From<rules::UserPage> for UserListResponse {
    fn from(page: rules::UserPage) -> Self {
        let message = page.message_text().map(str::to_owned);
        let rules::UserPage { rows, state, .. } = page;
        let rules::PagingState {
            first_key,
            last_key,
            page_no,
            next_page,
        } = state;
        Self {
            rows: match rows {
                rules::PageRows::Replace(positions) => Some(
                    positions
                        .into_iter()
                        .map(|position| position.map(UserListRow::from))
                        .collect(),
                ),
                rules::PageRows::Keep => None,
            },
            page_no: page_no.get(),
            first_key: shown_key(&first_key),
            last_key: shown_key(&last_key),
            has_next_page: next_page.is_yes(),
            message,
        }
    }
}

/// A paging key as the response carries it: its text, `None` when blank.
fn shown_key(key: &PicX<8>) -> Option<String> {
    let shown = text(key);
    (!shown.is_empty()).then_some(shown)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::{FromRequestParts, Request};
    use axum::http::StatusCode;
    use domain::copybook::{PicU, PicX};
    use domain::online::cousr00c as rules;
    use utoipa::{IntoParams, ToSchema};

    use super::{UserListQuery, UserListResponse, UserListRow};
    use crate::dto::{
        FIELD_LENGTH_MESSAGE, FIELD_RANGE_MESSAGE, INVALID_QUERY, PAGE_SIZE_FIELD,
        PAGE_SIZE_MESSAGE, PageDirection, ValidQuery,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The ten in-stream users of `DUSRSECJ.jcl:35-44`, in key order: user ID, first name, last
    /// name, type.
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

    fn domain_row(id: &str, first: &str, last: &str, user_type: &str) -> rules::UserListRow {
        rules::row_from(
            key(id),
            PicX::moved_from(first.as_bytes()),
            PicX::moved_from(last.as_bytes()),
            PicX::moved_from(user_type.as_bytes()),
        )
    }

    fn fixture() -> Vec<rules::UserListRow> {
        FIXTURE
            .iter()
            .map(|&(id, first, last, user_type)| domain_row(id, first, last, user_type))
            .collect()
    }

    /// The answer the domain gives `query` over `users`, read as the page queries read: at or
    /// above the key ascending, at or below it descending, at most the fetch limit.
    fn answer(
        query: &UserListQuery,
        users: &[rules::UserListRow],
    ) -> Result<rules::UserPage, rules::UserListFailure> {
        match rules::plan(
            query.action(),
            query.paging_state(),
            query.resolved_page_size(),
        )? {
            rules::Plan::Respond(page) => Ok(page),
            rules::Plan::Read(read) => {
                let at = read.key().clone();
                let rows: Vec<rules::UserListRow> = match read.direction() {
                    rules::Direction::Forward => users
                        .iter()
                        .filter(|user| user.user_id >= at)
                        .take(read.fetch_limit())
                        .cloned()
                        .collect(),
                    rules::Direction::Backward => users
                        .iter()
                        .rev()
                        .filter(|user| user.user_id <= at)
                        .take(read.fetch_limit())
                        .cloned()
                        .collect(),
                };
                rules::page(read, rows)
            }
        }
    }

    /// The query a screen sends after `response`: its paging state echoed, in `direction`.
    fn echo(
        response: &UserListResponse,
        direction: Option<PageDirection>,
    ) -> Result<UserListQuery, Box<dyn StdError>> {
        let page_key = match direction {
            Some(PageDirection::Forward) => response.last_key.clone(),
            Some(PageDirection::Backward) => response.first_key.clone(),
            None => None,
        };
        Ok(UserListQuery {
            page_key,
            direction,
            first_key: response.first_key.clone(),
            last_key: response.last_key.clone(),
            // TRUNCATION: checked, never lossy. The page number is PIC 9(08), at most
            // 99,999,999, which `i64` holds; `?` refuses any other value, never wrapping it.
            page_no: Some(i64::try_from(response.page_no)?),
            has_next_page: Some(response.has_next_page),
            ..UserListQuery::default()
        })
    }

    fn user_ids(response: &UserListResponse) -> Vec<Option<&str>> {
        response
            .rows
            .iter()
            .flatten()
            .map(|row| row.as_ref().map(|row| row.user_id.as_str()))
            .collect()
    }

    async fn query(uri: &str) -> Result<Result<UserListQuery, ScreenError>, axum::http::Error> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<UserListQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(value)| value),
        )
    }

    #[test]
    fn schema_names_are_the_generated_type_names() {
        assert_eq!(UserListQuery::name(), "UserListQuery");
        assert_eq!(UserListRow::name(), "UserListRow");
        assert_eq!(UserListResponse::name(), "UserListResponse");
    }

    #[test]
    fn every_query_member_is_an_optional_query_parameter_with_its_bounds() -> TestResult {
        let parameters = serde_json::to_value(UserListQuery::into_params(|| None))?;
        let parameters = parameters.as_array().ok_or("parameters are not a list")?;
        let names: Vec<&str> = parameters
            .iter()
            .filter_map(|parameter| parameter["name"].as_str())
            .collect();
        assert_eq!(
            names,
            [
                "start_user_id",
                "page_key",
                "direction",
                "first_key",
                "last_key",
                "page_no",
                "has_next_page",
                "page_size"
            ]
        );
        for parameter in parameters {
            assert_eq!(parameter["in"], "query", "{parameter}");
            assert_ne!(parameter["required"], true, "{parameter}");
        }
        let schema = |name: &str| {
            parameters
                .iter()
                .find(|parameter| parameter["name"] == name)
                .map(|parameter| parameter["schema"].clone())
                .ok_or(format!("no parameter {name}"))
        };
        let page_size = schema("page_size")?;
        assert_eq!(
            (&page_size["minimum"], &page_size["maximum"]),
            (&serde_json::json!(1), &serde_json::json!(100))
        );
        let page_no = schema("page_no")?;
        assert_eq!(
            (&page_no["minimum"], &page_no["maximum"]),
            (&serde_json::json!(0), &serde_json::json!(99_999_999))
        );
        for name in ["start_user_id", "page_key", "first_key", "last_key"] {
            assert_eq!(schema(name)?["maxLength"], 8, "{name}");
        }
        Ok(())
    }

    #[test]
    fn a_row_carries_the_four_fields_trimmed_and_no_password() -> TestResult {
        let row = UserListRow::from(domain_row("USER0001", "LAWRENCE", "THOMAS", "U"));
        assert_eq!(
            serde_json::to_value(&row)?,
            serde_json::json!({
                "user_id": "USER0001",
                "first_name": "LAWRENCE",
                "last_name": "THOMAS",
                "user_type": "U"
            })
        );
        let short = UserListRow::from(domain_row("U1", " LEE", "", ""));
        assert_eq!(
            (short.user_id.as_str(), short.first_name.as_str()),
            ("U1", " LEE")
        );
        assert!(short.last_name.is_empty() && short.user_type.is_empty());
        Ok(())
    }

    #[test]
    fn the_first_entry_lists_the_fixture_from_the_lowest_user_id() -> TestResult {
        // audit: COUSR00C-013
        let response = UserListResponse::from(answer(&UserListQuery::default(), &fixture())?);
        let expected: Vec<Option<&str>> = FIXTURE.iter().map(|&(id, ..)| Some(id)).collect();
        assert_eq!(user_ids(&response), expected);
        // audit: COUSR00C-014
        assert_eq!(response.page_no, 1);
        assert!(!response.has_next_page);
        assert_eq!(
            response.message.as_deref(),
            Some("You have reached the bottom of the page...")
        );
        // audit: COUSR00C-016
        assert_eq!(response.first_key.as_deref(), Some("ADMIN001"));
        assert_eq!(response.last_key.as_deref(), Some("USER0005"));
        let json = serde_json::to_string(&response)?;
        assert!(
            !json.contains("password") && !json.contains("pwd"),
            "{json}"
        );
        Ok(())
    }

    #[test]
    fn enter_keeps_the_echoed_last_key_when_row_ten_stays_blank() -> TestResult {
        let first = UserListResponse::from(answer(&UserListQuery::default(), &fixture())?);
        let search = UserListQuery {
            start_user_id: Some("USER0001".to_owned()),
            ..echo(&first, None)?
        };
        let response = UserListResponse::from(answer(&search, &fixture())?);
        // audit: COUSR00C-015
        assert_eq!(
            user_ids(&response),
            [
                Some("USER0001"),
                Some("USER0002"),
                Some("USER0003"),
                Some("USER0004"),
                Some("USER0005"),
                None,
                None,
                None,
                None,
                None
            ]
        );
        assert_eq!(response.page_no, 1);
        // audit: COUSR00C-016
        assert_eq!(response.first_key.as_deref(), Some("USER0001"));
        assert_eq!(response.last_key.as_deref(), Some("USER0005"));
        Ok(())
    }

    #[test]
    fn f7_then_f8_on_the_only_page_gives_the_audited_pair() -> TestResult {
        let first = UserListResponse::from(answer(&UserListQuery::default(), &fixture())?);

        // audit: COUSR00C-021
        let top = UserListResponse::from(answer(
            &echo(&first, Some(PageDirection::Backward))?,
            &fixture(),
        )?);
        assert_eq!(top.rows, None);
        assert!(top.has_next_page);
        assert_eq!(
            (
                top.page_no,
                top.first_key.as_deref(),
                top.last_key.as_deref()
            ),
            (1, Some("ADMIN001"), Some("USER0005"))
        );
        assert_eq!(
            top.message.as_deref(),
            Some("You are already at the top of the page...")
        );

        // audit: COUSR00C-018
        // audit: COUSR00C-019
        let bottom = UserListResponse::from(answer(
            &echo(&top, Some(PageDirection::Forward))?,
            &fixture(),
        )?);
        assert_eq!(user_ids(&bottom), [None; 10]);
        assert!(!bottom.has_next_page);
        assert_eq!(
            (
                bottom.page_no,
                bottom.first_key.as_deref(),
                bottom.last_key.as_deref()
            ),
            (1, Some("ADMIN001"), Some("USER0005"))
        );
        assert_eq!(
            bottom.message.as_deref(),
            Some("You have reached the bottom of the page...")
        );

        // audit: COUSR00C-020
        let again = UserListResponse::from(answer(
            &echo(&bottom, Some(PageDirection::Forward))?,
            &fixture(),
        )?);
        assert_eq!(again.rows, None);
        assert_eq!(
            again.message.as_deref(),
            Some("You are already at the bottom of the page...")
        );
        Ok(())
    }

    #[test]
    fn eleven_users_page_forward_then_back_to_the_top() -> TestResult {
        let mut users = fixture();
        users.push(domain_row("ZUSER011", "ELEVENTH", "USER", "U"));
        let page_size = Some(5);
        let first = UserListResponse::from(answer(
            &UserListQuery {
                page_size,
                ..UserListQuery::default()
            },
            &users,
        )?);
        assert_eq!((first.page_no, first.has_next_page), (1, true));

        let second = UserListResponse::from(answer(
            &UserListQuery {
                page_size,
                ..echo(&first, Some(PageDirection::Forward))?
            },
            &users,
        )?);
        // audit: COUSR00C-017
        assert_eq!(
            user_ids(&second),
            [
                Some("USER0001"),
                Some("USER0002"),
                Some("USER0003"),
                Some("USER0004"),
                Some("USER0005")
            ]
        );
        assert_eq!((second.page_no, second.has_next_page), (2, true));

        // audit: COUSR00C-022
        // audit: COUSR00C-023
        let back = UserListResponse::from(answer(
            &UserListQuery {
                page_size,
                ..echo(&second, Some(PageDirection::Backward))?
            },
            &users,
        )?);
        assert_eq!(
            user_ids(&back),
            [
                Some("ADMIN001"),
                Some("ADMIN002"),
                Some("ADMIN003"),
                Some("ADMIN004"),
                Some("ADMIN005")
            ]
        );
        assert_eq!(back.page_no, 1);
        assert_eq!(
            back.message.as_deref(),
            Some("You have reached the top of the page...")
        );
        Ok(())
    }

    #[test]
    fn a_short_backward_page_leaves_its_first_rows_blank() -> TestResult {
        // audit: COUSR00C-024
        let query = UserListQuery {
            direction: Some(PageDirection::Backward),
            page_key: Some("ADMIN004".to_owned()),
            last_key: Some("USER0003".to_owned()),
            page_no: Some(3),
            has_next_page: Some(true),
            ..UserListQuery::default()
        };
        let response = UserListResponse::from(answer(&query, &fixture())?);
        let mut expected = vec![None; 7];
        expected.extend([Some("ADMIN001"), Some("ADMIN002"), Some("ADMIN003")]);
        assert_eq!(user_ids(&response), expected);
        assert_eq!(response.page_no, 3);
        assert_eq!(response.first_key.as_deref(), Some("ADMIN004"));
        assert_eq!(response.last_key.as_deref(), Some("ADMIN003"));
        Ok(())
    }

    #[test]
    fn the_query_maps_onto_the_domain_interaction_and_state() -> TestResult {
        let cases = [
            (
                UserListQuery::default(),
                rules::Action::Enter {
                    start: rules::LOW_KEY,
                },
            ),
            (
                UserListQuery {
                    start_user_id: Some("   ".to_owned()),
                    ..UserListQuery::default()
                },
                rules::Action::Enter {
                    start: rules::LOW_KEY,
                },
            ),
            (
                UserListQuery {
                    start_user_id: Some("user0001".to_owned()),
                    ..UserListQuery::default()
                },
                rules::Action::Enter {
                    start: key("user0001"),
                },
            ),
            (
                UserListQuery {
                    start_user_id: Some("USER0001".to_owned()),
                    direction: Some(PageDirection::Forward),
                    ..UserListQuery::default()
                },
                rules::Action::Page(rules::Direction::Forward),
            ),
            (
                UserListQuery {
                    direction: Some(PageDirection::Backward),
                    ..UserListQuery::default()
                },
                rules::Action::Page(rules::Direction::Backward),
            ),
        ];
        for (query, action) in cases {
            assert_eq!(query.action(), action, "{query:?}");
        }

        assert_eq!(
            UserListQuery::default().paging_state(),
            rules::PagingState::default()
        );
        let echoed = |direction| UserListQuery {
            direction,
            page_key: Some("PAGEKEY".to_owned()),
            first_key: Some("FIRST".to_owned()),
            last_key: Some("LAST".to_owned()),
            page_no: Some(4),
            has_next_page: Some(true),
            ..UserListQuery::default()
        };
        let state = |first: &str, last: &str| -> Result<rules::PagingState, Box<dyn StdError>> {
            Ok(rules::PagingState {
                first_key: key(first),
                last_key: key(last),
                page_no: PicU::new(4)?,
                next_page: rules::NextPage::Yes,
            })
        };
        assert_eq!(
            echoed(Some(PageDirection::Forward)).paging_state(),
            state("FIRST", "PAGEKEY")?
        );
        assert_eq!(
            echoed(Some(PageDirection::Backward)).paging_state(),
            state("PAGEKEY", "LAST")?
        );
        assert_eq!(echoed(None).paging_state(), state("FIRST", "LAST")?);
        for has_next_page in [None, Some(false)] {
            let query = UserListQuery {
                has_next_page,
                ..UserListQuery::default()
            };
            assert_eq!(query.paging_state().next_page, rules::NextPage::No);
        }
        Ok(())
    }

    #[test]
    fn the_page_size_defaults_to_the_map_rows() {
        for (requested, rows) in [
            (None, rules::DEFAULT_PAGE_SIZE),
            (Some(1), 1),
            (Some(100), 100),
        ] {
            let query = UserListQuery {
                page_size: requested,
                ..UserListQuery::default()
            };
            assert_eq!(query.resolved_page_size(), rows, "{requested:?}");
        }
    }

    #[tokio::test]
    async fn query_strings_are_validated_with_the_page_size_alone_answering_422() -> TestResult {
        let echoed = query(
            "/api/v1/users?direction=forward&page_key=USER0005&first_key=ADMIN001\
             &last_key=USER0005&page_no=1&has_next_page=true&page_size=10",
        )
        .await?
        .map_err(|error| error.to_string())?;
        assert_eq!(echoed.direction, Some(PageDirection::Forward));
        assert_eq!(echoed.has_next_page, Some(true));
        assert_eq!(echoed.page_no, Some(1));
        let first_entry = query("/api/v1/users?page_no=0")
            .await?
            .map_err(|error| error.to_string())?;
        assert_eq!(first_entry.page_no, Some(0));

        let cases = [
            (
                "/api/v1/users?page_size=101",
                StatusCode::UNPROCESSABLE_ENTITY,
                PAGE_SIZE_MESSAGE,
                Some(PAGE_SIZE_FIELD),
            ),
            (
                "/api/v1/users?page_size=0&start_user_id=USER00010",
                StatusCode::UNPROCESSABLE_ENTITY,
                PAGE_SIZE_MESSAGE,
                Some(PAGE_SIZE_FIELD),
            ),
            // audit: COUSR00-015
            (
                "/api/v1/users?start_user_id=USER00010",
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("start_user_id"),
            ),
            (
                "/api/v1/users?direction=backward&page_key=ADMIN0010",
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("page_key"),
            ),
            (
                "/api/v1/users?page_no=100000000",
                StatusCode::BAD_REQUEST,
                FIELD_RANGE_MESSAGE,
                Some("page_no"),
            ),
            (
                "/api/v1/users?page_no=-1",
                StatusCode::BAD_REQUEST,
                FIELD_RANGE_MESSAGE,
                Some("page_no"),
            ),
            (
                "/api/v1/users?direction=sideways",
                StatusCode::BAD_REQUEST,
                INVALID_QUERY,
                None,
            ),
            (
                "/api/v1/users?has_next_page=Y",
                StatusCode::BAD_REQUEST,
                INVALID_QUERY,
                None,
            ),
        ];
        for (uri, status, message, field) in cases {
            let error = query(uri).await?.err().ok_or(uri)?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (status, message, field),
                "{uri}"
            );
        }
        Ok(())
    }
}
