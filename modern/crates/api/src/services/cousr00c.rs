//! COUSR00C, the user list: the service of endpoint 16, `GET /api/v1/users` \[A\] (transaction
//! `CU00`, map `COUSR0A`; AAP 0.3.4.1).
//!
//! # The browse it replaces
//!
//! COUSR00C pages through the USRSEC KSDS with `STARTBR` (`COUSR00C.cbl:586-614`), `READNEXT`
//! (`COUSR00C.cbl:619-648`) or `READPREV` (`COUSR00C.cbl:653-682`) and `ENDBR`
//! (`COUSR00C.cbl:687-691`). Here one key-ordered page query of `users` takes the place of each
//! browse:
//!
//! 1. [`rules::plan`] turns the request's interaction (ENTER from the search field, F8 or F7),
//!    the paging state it echoes and its page size into a [`Plan`]. F8 with no next page and F7
//!    on the first page answer without a read (COUSR00C-020, COUSR00C-021).
//! 2. A [`Plan::Read`] names the read key, the direction and the fetch limit: the page, the user
//!    read ahead and, for F8 and F7, the record the browse discards first (COUSR00C-013,
//!    COUSR00C-014, COUSR00C-017, COUSR00C-022, COUSR00C-023). The service reads that many
//!    users at most, ascending through [`UserRepository::page_after`] for ENTER and F8 and
//!    descending through [`UserRepository::page_before`] for F7, the key itself included.
//! 3. The rows go unchanged, in the order read, to [`rules::page`], which fills the row
//!    positions, the paging state and the message.
//!
//! The start key, the page messages and the halted browses (COUSR00C-025, COUSR00C-027) are the
//! domain's; this module decides none of them.
//!
//! # Failures
//!
//! A failing page query is the `WHEN OTHER` of the three browse paragraphs: the answer is
//! [`UserListFailure::LookupFailed`], `Unable to lookup User...` with status 500 (COUSR00C-026).
//! The COBOL `DISPLAY`s the response codes to the region log; here the error's SQLSTATE and
//! constraint name, which hold no stored value, go to the log as one event.
//!
//! # Rules
//!
//! - **Admin gate** (COUSR00C-001): `AdminUser` refuses a missing token with 401 and a user
//!   type other than `A` with 403 before this service runs. The search value and the paging
//!   state are request data and grant nothing (AAP 0.5.1).
//! - **Bounded Pagination:** the page size is the one `UserListQuery` validated (1 to 100,
//!   default 10), and every read goes through the two page queries
//!   (`queries/users_page_forward.sql`, `queries/users_page_backward.sql`) with the domain's
//!   fetch limit.
//! - **Cardholder Data Redaction:** the page queries select the user ID, names and type only,
//!   so no hash is read, and the rows carry no password (COUSR00C-028).
//! - **Thin Request Handlers:** the handler extracts, calls [`list`] and maps the returned
//!   [`UserPage`] to `UserListResponse`; every branch on the plan and the read lives here.

use domain::online::cousr00c::{self as rules, Direction, Plan, UserListFailure, UserPage};
use persistence::UserRepository;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cousr00c::UserListQuery;
use crate::error::ServiceError;

/// Endpoint 16: the page of users `query` asks for, with its paging state and message
/// (COUSR00C-012 to COUSR00C-024, COUSR00C-028).
///
/// `_admin` is the gate the handler's extractor has already passed (COUSR00C-001); the list
/// depends on no part of the caller's identity.
///
/// # Errors
///
/// [`ServiceError::Rule`] with [`UserListFailure::LookupFailed`] when a page query fails or
/// returns users out of read order (COUSR00C-026), and for the halted F8 the domain refuses
/// without a read (COUSR00C-027); [`ServiceError::Internal`] when the fetch limit does not fit
/// the query's `BIGINT` bind, which no validated page size reaches.
pub(crate) async fn list(
    state: &AppState,
    _admin: &AdminUser,
    query: UserListQuery,
) -> Result<UserPage, ServiceError<UserListFailure>> {
    // A clone shares the pool; the page queries need the `&mut` receiver the trait declares.
    let mut store = state.store.clone();
    browse(&mut store, query).await
}

/// The interaction of `query` against `repo`: plan, at most one page read, then the page.
async fn browse<R: UserRepository>(
    repo: &mut R,
    query: UserListQuery,
) -> Result<UserPage, ServiceError<UserListFailure>> {
    let planned = rules::plan(
        query.action(),
        query.paging_state(),
        query.resolved_page_size(),
    )
    .map_err(ServiceError::Rule)?;
    let read = match planned {
        Plan::Respond(page) => return Ok(page),
        Plan::Read(read) => read,
    };
    // TRUNCATION: checked, never lossy. The limit is a validated page size of 1 to 100 plus at
    // most two, which `i64` holds exactly; a limit it could not hold is refused, never clamped.
    let limit = i64::try_from(read.fetch_limit()).map_err(|_| ServiceError::Internal)?;
    let fetched = match read.direction() {
        Direction::Forward => UserRepository::page_after(repo, read.key(), limit).await,
        Direction::Backward => UserRepository::page_before(repo, read.key(), limit).await,
    };
    let entries = fetched.map_err(|error| {
        tracing::error!(error = %error, "COUSR00C user browse failed");
        ServiceError::Rule(UserListFailure::LookupFailed)
    })?;
    let rows = entries.into_iter().map(rules::UserListRow::from).collect();
    rules::page(read, rows).map_err(ServiceError::Rule)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::future::ready;

    use axum::http::StatusCode;
    use domain::batch::loads::HashedUser;
    use domain::copybook::PicX;
    use domain::online::cousr00c::{
        LOW_KEY, ListMessage, NextPage, PageRows, UserListFailure, UserPage,
    };
    use domain::records::{UserEntry, UserListEntry};
    use persistence::{PersistenceError, UserRepository};

    use super::browse;
    use crate::dto::PageDirection;
    use crate::dto::cousr00c::UserListQuery;
    use crate::error::{ScreenError, ServiceError};

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

    /// One call the service made of the store.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        PageAfter(PicX<8>, i64),
        PageBefore(PicX<8>, i64),
        /// A method other than the two page reads, by name.
        Other(&'static str),
    }

    /// An in-memory `users` table that answers the two page reads as the page queries do (the
    /// start key included, at most `limit` users, ascending or descending) and records every
    /// call. The user list makes no other call: the double refuses the rest as unavailable and
    /// records them, so a stray call shows in the call log a test asserts.
    struct ListStore {
        users: Vec<UserListEntry>,
        fail: bool,
        calls: Vec<Call>,
    }

    impl ListStore {
        fn new(users: Vec<UserListEntry>) -> Self {
            Self {
                users,
                fail: false,
                calls: Vec::new(),
            }
        }

        fn failing() -> Self {
            Self {
                fail: true,
                ..Self::new(fixture())
            }
        }

        /// The users a page query returns, or the failure of a store that cannot be read.
        fn page<'u>(
            &self,
            matching: impl Iterator<Item = &'u UserListEntry>,
            limit: i64,
        ) -> Result<Vec<UserListEntry>, PersistenceError> {
            if self.fail {
                return Err(PersistenceError::Unavailable);
            }
            // TRUNCATION: checked, never lossy. `PageRead::fetch_limit` is a page size of 1 to 100
            // plus the read-ahead and the discarded start record, at most 102, which `usize`
            // holds; a negative `LIMIT` is refused as unavailable, never wrapped.
            let limit = usize::try_from(limit).map_err(|_| PersistenceError::Unavailable)?;
            Ok(matching.take(limit).cloned().collect())
        }

        fn refuse<T>(&mut self, method: &'static str) -> Result<T, PersistenceError> {
            self.calls.push(Call::Other(method));
            Err(PersistenceError::Unavailable)
        }
    }

    impl UserRepository for ListStore {
        fn find_by_id(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            ready(self.refuse("find_by_id"))
        }

        fn lock_by_id(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            ready(self.refuse("lock_by_id"))
        }

        fn page_after(
            &mut self,
            start: &PicX<8>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
            self.calls.push(Call::PageAfter(start.clone(), limit));
            ready(self.page(self.users.iter().filter(|user| user.id >= *start), limit))
        }

        fn page_before(
            &mut self,
            start: &PicX<8>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
            self.calls.push(Call::PageBefore(start.clone(), limit));
            ready(self.page(
                self.users.iter().rev().filter(|user| user.id <= *start),
                limit,
            ))
        }

        fn insert(
            &mut self,
            _entry: &UserEntry,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            ready(self.refuse("insert"))
        }

        fn update(
            &mut self,
            _entry: &UserEntry,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            ready(self.refuse("update"))
        }

        fn delete(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            ready(self.refuse("delete"))
        }

        fn replace_all(
            &mut self,
            _users: &[HashedUser],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            ready(self.refuse("replace_all"))
        }
    }

    fn key(text: &str) -> PicX<8> {
        PicX::moved_from(text.as_bytes())
    }

    fn entry(id: &str, first: &str, last: &str, user_type: &str) -> UserListEntry {
        UserListEntry {
            id: key(id),
            fname: PicX::moved_from(first.as_bytes()),
            lname: PicX::moved_from(last.as_bytes()),
            usr_type: PicX::moved_from(user_type.as_bytes()),
        }
    }

    fn fixture() -> Vec<UserListEntry> {
        FIXTURE
            .iter()
            .map(|&(id, first, last, user_type)| entry(id, first, last, user_type))
            .collect()
    }

    /// The fixture and an eleventh user after it.
    fn eleven_users() -> Vec<UserListEntry> {
        let mut users = fixture();
        users.push(entry("ZUSER011", "ELEVENTH", "USER", "U"));
        users
    }

    /// The user IDs of the filled rows, first to last.
    fn filled_ids(page: &UserPage) -> Vec<String> {
        page.rows
            .filled()
            .map(|row| row.user_id.to_text())
            .collect()
    }

    /// An F8 or F7 request echoing the paging state of a page numbered `page_no`.
    fn paging(direction: PageDirection, page_key: &str, page_no: i64) -> UserListQuery {
        UserListQuery {
            direction: Some(direction),
            page_key: Some(page_key.to_owned()),
            page_no: Some(page_no),
            has_next_page: Some(true),
            ..UserListQuery::default()
        }
    }

    // audit: COUSR00C-012
    // audit: COUSR00C-013
    // audit: COUSR00C-014
    // audit: COUSR00C-028
    #[tokio::test]
    async fn enter_without_a_search_value_reads_ten_users_and_one_ahead_from_the_low_key()
    -> TestResult {
        let mut store = ListStore::new(fixture());
        let page = browse(&mut store, UserListQuery::default()).await?;
        assert_eq!(store.calls, [Call::PageAfter(LOW_KEY, 11)]);
        let ids: Vec<&str> = FIXTURE.iter().map(|&(id, ..)| id).collect();
        assert_eq!(filled_ids(&page), ids);
        let first = page.rows.filled().next().ok_or("row 1 is filled")?;
        assert_eq!(first.first_name.to_text(), "MARGARET");
        assert_eq!(first.last_name.to_text(), "GOLD");
        assert_eq!(first.user_type.to_text(), "A");
        assert_eq!(page.state.page_no.get(), 1);
        assert_eq!(page.state.first_key, key("ADMIN001"));
        assert_eq!(page.state.last_key, key("USER0005"));
        assert_eq!(page.state.next_page, NextPage::No);
        assert_eq!(page.message, Some(ListMessage::ReachedBottom));
        Ok(())
    }

    // audit: COUSR00C-012
    // audit: COUSR00C-015
    #[tokio::test]
    async fn enter_reads_from_the_search_value_as_typed() -> TestResult {
        let mut store = ListStore::new(fixture());
        let query = UserListQuery {
            start_user_id: Some("USER0001".to_owned()),
            ..UserListQuery::default()
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(store.calls, [Call::PageAfter(key("USER0001"), 11)]);
        assert_eq!(
            filled_ids(&page),
            ["USER0001", "USER0002", "USER0003", "USER0004", "USER0005"]
        );
        assert_eq!(page.state.page_no.get(), 1);
        assert_eq!(page.message, Some(ListMessage::ReachedBottom));
        Ok(())
    }

    // audit: COUSR00C-017
    // audit: COUSR00C-014
    #[tokio::test]
    async fn f8_reads_the_discarded_record_the_page_and_one_ahead_at_the_page_size() -> TestResult {
        let mut store = ListStore::new(eleven_users());
        let query = UserListQuery {
            page_size: Some(5),
            ..paging(PageDirection::Forward, "ADMIN005", 1)
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(store.calls, [Call::PageAfter(key("ADMIN005"), 7)]);
        assert_eq!(
            filled_ids(&page),
            ["USER0001", "USER0002", "USER0003", "USER0004", "USER0005"]
        );
        assert_eq!(page.state.page_no.get(), 2);
        assert_eq!(page.state.next_page, NextPage::Yes);
        assert_eq!(page.message, None);
        Ok(())
    }

    // audit: COUSR00C-022
    // audit: COUSR00C-023
    #[tokio::test]
    async fn f7_reads_backward_from_the_first_user_id_shown() -> TestResult {
        let mut store = ListStore::new(eleven_users());
        let query = UserListQuery {
            page_size: Some(5),
            ..paging(PageDirection::Backward, "USER0001", 2)
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(store.calls, [Call::PageBefore(key("USER0001"), 7)]);
        assert_eq!(
            filled_ids(&page),
            ["ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005"]
        );
        assert_eq!(page.state.page_no.get(), 1);
        assert_eq!(page.state.next_page, NextPage::Yes);
        assert_eq!(page.message, Some(ListMessage::ReachedTop));
        Ok(())
    }

    // audit: COUSR00C-020
    // audit: COUSR00C-021
    #[tokio::test]
    async fn f8_without_a_next_page_and_f7_on_page_one_read_nothing() -> TestResult {
        let cases = [
            (
                UserListQuery {
                    has_next_page: Some(false),
                    ..paging(PageDirection::Forward, "USER0005", 1)
                },
                ListMessage::AlreadyAtBottom,
            ),
            (
                paging(PageDirection::Backward, "ADMIN001", 1),
                ListMessage::AlreadyAtTop,
            ),
        ];
        for (query, message) in cases {
            let mut store = ListStore::new(fixture());
            let page = browse(&mut store, query).await?;
            assert_eq!(store.calls, []);
            assert_eq!(page.rows, PageRows::Keep);
            assert_eq!(page.message, Some(message));
        }
        Ok(())
    }

    // audit: COUSR00C-026
    #[tokio::test]
    async fn a_failing_page_read_is_the_cobol_lookup_failure_with_status_500() -> TestResult {
        let cases = [
            (UserListQuery::default(), Call::PageAfter(LOW_KEY, 11)),
            (
                paging(PageDirection::Backward, "USER0001", 2),
                Call::PageBefore(key("USER0001"), 12),
            ),
        ];
        for (query, call) in cases {
            let mut store = ListStore::failing();
            let failure = browse(&mut store, query)
                .await
                .err()
                .ok_or("a failing read answers a failure")?;
            assert_eq!(store.calls, [call]);
            assert_eq!(failure, ServiceError::Rule(UserListFailure::LookupFailed));
            let error = ScreenError::from(failure);
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.message(), "Unable to lookup User...");
            assert_eq!(error.field(), None);
        }
        Ok(())
    }
}
