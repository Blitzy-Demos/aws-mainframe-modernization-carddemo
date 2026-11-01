//! `COUSR00C` (CU00), the user list: ten `USRSEC` users per page in
//! `SEC-USR-ID` order, served as
//! `GET /admin/users?start_key=&direction=&page_size=`.
//!
//! The COBOL keeps its page state in the COMMAREA: the first and last user ID
//! shown (`CDEMO-CU00-USRID-FIRST`, `CDEMO-CU00-USRID-LAST`), a page number
//! and the next-page flag [app/cbl/COUSR00C.cbl:L67-75]. The API keeps none, so
//! a request carries the start key and the page carries the keys that
//! reproduce the next PF8 and PF7 pages:
//!
//! - [`Direction::Forward`] is ENTER (`PROCESS-ENTER-KEY`, F-COUSR00-001):
//!   the page starts at the first user ID at or after `start_key`, from the
//!   top of the file when the key is absent or blank. Its
//!   [`UserListPage::next_key`] is the user the look-ahead `READNEXT` found,
//!   so a forward request from it shows exactly the rows PF8 shows after it
//!   discards the record at the last key shown (P-COUSR00C-003), and no row
//!   repeats.
//! - [`Direction::Backward`] is PF7 (`PROCESS-PAGE-BACKWARD`, F-COUSR00-003)
//!   from the first key shown, [`UserListPage::prev_key`]: the record at the
//!   browse position is discarded and the page holds the users before it.
//!
//! The page-number and next-page-flag checks of PF7 and PF8 have no state to
//! test, so the audit maps them onto the keys (E-54): a forward request from
//! the last user ID is the PF8 without a next page (V-COUSR00C-002) and a
//! backward request from the first user ID the PF7 on page 1
//! (V-COUSR00C-007). Messages set without `WS-ERR-FLG` (the page-boundary
//! texts) come back in [`UserListPage::message`] of a successful page; the
//! `WS-ERR-FLG` paths are [`DomainError`]s on the BMS field `USRIDIN`.
//!
//! The row selectors `SEL0001`..`SEL0010` and their
//! `"Invalid selection. Valid values are U and D"` (P-COUSR00C-002,
//! V-COUSR00C-001) have no port here: no REST parameter carries a selector,
//! and the row actions of the user list open the CU02 edit route and the CU03
//! delete dialog instead (E-54, F-COUSR00-005, F-COUSR00-006). The 3270
//! header, the screen sends and the receive (P-COUSR00C-013 to
//! P-COUSR00C-015) and the PF3 return (P-COUSR00C-012) are navigation, not
//! rules.

use crate::entities::UserSecurity;
use crate::error::DomainError;
use crate::messages;
use crate::numeric::UserId;
use crate::ports::{Browse, Direction, UserStore};

/// The BMS field every COUSR00C message concerns: each path moves -1 to
/// `USRIDINL`, so the cursor stays on the search field.
const USRIDIN: &str = "USRIDIN";

/// The rows of one COUSR00 page: `USER-REC OCCURS 10 TIMES`
/// [app/cbl/COUSR00C.cbl:L56-64], the default `page_size` of the list.
pub const SCREEN_ROWS: u16 = 10;

/// One user-list request: the query of `GET /admin/users`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListInput {
    /// The user ID the page starts from, as entered in `USRIDIN` or taken from
    /// a page's [`UserListPage::next_key`] or [`UserListPage::prev_key`].
    /// Absent, empty, all spaces or all low-values starts at the top of the
    /// file (`LOW-VALUES`). The API rejects a key longer than `X(08)` before
    /// it reaches the rule; a longer one is cut at 8 bytes, as the `MOVE` does.
    pub start_key: Option<String>,
    /// Forward is ENTER (from the key), backward is PF7 (before the key).
    pub direction: Direction,
    /// The most users on the page; the API passes [`SCREEN_ROWS`] by default
    /// and 1 to 100 when the request sets it.
    pub page_size: u16,
}

/// One page of the user list, in ascending user-ID order as the screen shows
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListPage {
    /// The users shown, one item per filled row (P-COUSR00C-010,
    /// P-COUSR00C-011). The API publishes only their ID, names and type;
    /// `SEC-USR-PWD` is never shown.
    pub items: Vec<UserSecurity>,
    /// The start key of a forward request that shows the next page, when the
    /// COBOL would set `NEXT-PAGE-YES`: the user the look-ahead read found
    /// after a forward page, and the browse position after a backward page,
    /// where PF7 always sets the flag [app/cbl/COUSR00C.cbl:L245].
    pub next_key: Option<UserId>,
    /// The start key of a backward request that shows the previous page: the
    /// first user ID shown, `CDEMO-CU00-USRID-FIRST`.
    pub prev_key: Option<UserId>,
    /// The page-boundary text the last screen send carries, if any.
    pub message: Option<&'static str>,
}

impl UserListPage {
    /// A page that shows no user, only `message`.
    fn without_rows(message: &'static str) -> Self {
        Self {
            items: Vec::new(),
            next_key: None,
            prev_key: None,
            message: Some(message),
        }
    }
}

/// COUSR00C MAIN-PARA — P-COUSR00C-001, P-COUSR00C-003, P-COUSR00C-009;
/// V-COUSR00C-004, V-COUSR00C-006, V-COUSR00C-009.
///
/// The dispatch of ENTER and PF7 [app/cbl/COUSR00C.cbl:L122-129].
///
/// A forward request is the ENTER page (`PROCESS-PAGE-FORWARD`), a backward
/// request the PF7 page (`PROCESS-PAGE-BACKWARD`), as the module documentation
/// maps them.
///
/// # Errors
///
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
/// `"Unable to lookup User..."` on the field `USRIDIN` when the user browse
/// fails: `STARTBR` (V-COUSR00C-004), `READNEXT` (V-COUSR00C-006) or
/// `READPREV` (V-COUSR00C-009).
pub async fn list<S: UserStore>(
    store: &mut S,
    input: UserListInput,
) -> Result<UserListPage, DomainError> {
    let start = start_key(input.start_key.as_deref());
    match input.direction {
        Direction::Forward => page_forward(store, start, input.page_size).await,
        Direction::Backward => page_backward(store, start, input.page_size).await,
    }
}

/// COUSR00C PROCESS-ENTER-KEY, PROCESS-PF7-KEY — P-COUSR00C-003,
/// P-COUSR00C-009.
///
/// The start key [app/cbl/COUSR00C.cbl:L218-222]
/// [app/cbl/COUSR00C.cbl:L239-243]: `MOVE USRIDINI TO SEC-USR-ID`, or
/// `LOW-VALUES` (`None`) when the key is `SPACES` or `LOW-VALUES`. The key
/// keeps its case: `USRSEC` compares it byte for byte.
fn start_key(raw: Option<&str>) -> Option<UserId> {
    let key = UserId::move_from(raw?);
    (!key.as_alnum().is_blank()).then_some(key)
}

/// COUSR00C STARTBR-USER-SEC-FILE, READNEXT-USER-SEC-FILE,
/// READPREV-USER-SEC-FILE — P-COUSR00C-006, P-COUSR00C-007, P-COUSR00C-008;
/// V-COUSR00C-004, V-COUSR00C-006, V-COUSR00C-009.
///
/// One `browse_users` call: a `STARTBR` with its `READNEXT` or `READPREV`
/// reads [app/cbl/COUSR00C.cbl:L586-682].
///
/// # Errors
///
/// `"Unable to lookup User..."` on `USRIDIN` for any port failure: each of the
/// three paragraphs answers a `RESP` other than its `NOTFND` or `ENDFILE` that
/// way, with `WS-ERR-FLG` set.
async fn browse<S: UserStore>(
    store: &mut S,
    start_key: Option<UserId>,
    direction: Direction,
    limit: u16,
) -> Result<Vec<UserSecurity>, DomainError> {
    let browse = Browse {
        start_key,
        direction,
        limit,
    };
    store
        .browse_users(browse)
        .await
        .map_err(|_| DomainError::unexpected(messages::UNABLE_TO_LOOKUP_USER).with_field(USRIDIN))
}

/// COUSR00C PROCESS-PAGE-FORWARD — P-COUSR00C-003, P-COUSR00C-005,
/// P-COUSR00C-006, P-COUSR00C-007, P-COUSR00C-010, P-COUSR00C-011,
/// P-COUSR00C-016, P-COUSR00C-017; V-COUSR00C-002, V-COUSR00C-003,
/// V-COUSR00C-005.
///
/// The ENTER page [app/cbl/COUSR00C.cbl:L282-331]. `STARTBR` positions at
/// the first user ID at or after `start` (`GTEQ`, the default; the explicit
/// option is commented out [app/cbl/COUSR00C.cbl:L592]).
/// The loop reads up to `rows` users, then one look-ahead `READNEXT` decides
/// the next page [app/cbl/COUSR00C.cbl:L308-323]:
///
/// - no user at or after `start`: `STARTBR` `NOTFND`, an empty page with
///   `"You are at the top of the page..."` (V-COUSR00C-003; the COBOL then
///   abends on its `ENDBR`, which is not reproduced, E-71);
/// - `start` is the last user ID: the PF8 without a next page,
///   `"You are already at the bottom of the page..."`, which ranks first
///   (V-COUSR00C-002);
/// - the loop or the look-ahead meets the end of the file: the users read and
///   `"You have reached the bottom of the page..."` (V-COUSR00C-005);
/// - otherwise the full page, and the look-ahead user as the next key.
///
/// # Errors
///
/// Those of [`browse`].
async fn page_forward<S: UserStore>(
    store: &mut S,
    start: Option<UserId>,
    rows: u16,
) -> Result<UserListPage, DomainError> {
    let mut read = browse(store, start, Direction::Forward, rows.saturating_add(1)).await?;
    let Some(first) = read.first().map(|user| user.sec_usr_id) else {
        return Ok(UserListPage::without_rows(
            messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
        ));
    };
    if read.len() == 1 && start == Some(first) {
        // V-COUSR00C-002 maps the PF8 without a next page onto a forward
        // request from the last user ID; the COBOL browses nothing there and
        // leaves the rows it showed on screen. The page keeps that one user
        // instead of coming back empty, because a forward request from the
        // last user ID is also how a client follows a next key to a final
        // page that holds only that user, who must stay listable.
        return Ok(UserListPage {
            items: read,
            next_key: None,
            prev_key: Some(first),
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE),
        });
    }
    let shown = usize::from(rows);
    let next_key = read.get(shown).map(|user| user.sec_usr_id);
    read.truncate(shown);
    Ok(UserListPage {
        prev_key: read.first().map(|user| user.sec_usr_id),
        next_key,
        items: read,
        message: next_key
            .is_none()
            .then_some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE),
    })
}

/// COUSR00C PROCESS-PF7-KEY, PROCESS-PAGE-BACKWARD — P-COUSR00C-004,
/// P-COUSR00C-006, P-COUSR00C-008, P-COUSR00C-009, P-COUSR00C-010,
/// P-COUSR00C-011, P-COUSR00C-016, P-COUSR00C-017; V-COUSR00C-003,
/// V-COUSR00C-007, V-COUSR00C-008.
///
/// The PF7 page [app/cbl/COUSR00C.cbl:L237-255]
/// [app/cbl/COUSR00C.cbl:L336-379]. `STARTBR` positions at the first user ID
/// at or after `start`, the top of the file when it is absent. The first
/// `READPREV` returns the record at that position and is discarded; up to
/// `rows` more fill the page from its last row up, and after a full page one
/// more `READPREV` looks ahead:
///
/// - no user at or after `start`: `STARTBR` `NOTFND`, an empty page with
///   `"You are at the top of the page..."` (V-COUSR00C-003, E-71);
/// - no user before the position: the PF7 on the first page, an empty page
///   with `"You are already at the top of the page..."`, which ranks first
///   (V-COUSR00C-007);
/// - the loop or the look-ahead meets the start of the file: the users read
///   and `"You have reached the top of the page..."` (V-COUSR00C-008);
/// - otherwise the full page.
///
/// The next key is the browse position, the first user after the page.
///
/// # Errors
///
/// Those of [`browse`].
async fn page_backward<S: UserStore>(
    store: &mut S,
    start: Option<UserId>,
    rows: u16,
) -> Result<UserListPage, DomainError> {
    let Some(position) = browse(store, start, Direction::Forward, 1)
        .await?
        .first()
        .map(|user| user.sec_usr_id)
    else {
        return Ok(UserListPage::without_rows(
            messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
        ));
    };
    let mut read = browse(
        store,
        Some(position),
        Direction::Backward,
        rows.saturating_add(2),
    )
    .await?;
    // The record at the browse position is the one the first READPREV
    // returns and the paragraph discards. It is absent only when it was
    // deleted after the position was read; the users before it remain.
    if read.first().is_some_and(|user| user.sec_usr_id == position) {
        read.remove(0);
    }
    if read.is_empty() {
        return Ok(UserListPage::without_rows(
            messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
        ));
    }
    let shown = usize::from(rows);
    let look_ahead_found = read.len() > shown;
    read.truncate(shown);
    read.reverse();
    Ok(UserListPage {
        prev_key: read.first().map(|user| user.sec_usr_id),
        next_key: Some(position),
        items: read,
        message: (!look_ahead_found).then_some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
    })
}

#[cfg(test)]
mod tests {
    use super::{SCREEN_ROWS, USRIDIN, UserListInput, UserListPage, list};
    use crate::entities::UserSecurity;
    use crate::error::{DomainError, ErrorKind};
    use crate::messages;
    use crate::numeric::{Alnum, UserId};
    use crate::ports::{Browse, Direction, PortError, PortResult, UserStore};
    use crate::rules::test_support::{FakeStore, block_on};

    use Direction::{Backward, Forward};

    /// The ten DUSRSECJ users, ADMIN001-ADMIN005 and USER0001-USER0005
    /// (P-CSUSR01Y-007 to P-CSUSR01Y-010).
    const SEEDED: [&str; 10] = [
        "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "USER0001", "USER0002",
        "USER0003", "USER0004", "USER0005",
    ];

    /// The seeded users and three more, in `SEC-USR-ID` order: `BUSER001`
    /// sorts between the admins and the users, the other two after them.
    const THIRTEEN: [&str; 13] = [
        "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "BUSER001", "USER0001",
        "USER0002", "USER0003", "USER0004", "USER0005", "USER0006", "ZUSER001",
    ];

    /// A well-formed argon2id PHC string with a made-up salt and output.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";

    fn user(id: &str) -> UserSecurity {
        let user_type = if id.starts_with("ADMIN") { "A" } else { "U" };
        UserSecurity {
            sec_usr_id: UserId::move_from(id),
            sec_usr_fname: Alnum::move_from("FIRST"),
            sec_usr_lname: Alnum::move_from(id),
            password_hash: HASH.to_owned(),
            sec_usr_type: Alnum::move_from(user_type),
        }
    }

    fn store(ids: &[&str]) -> FakeStore {
        ids.iter()
            .fold(FakeStore::default(), |store, id| store.with_user(user(id)))
    }

    fn input(start: Option<&str>, direction: Direction, page_size: u16) -> UserListInput {
        UserListInput {
            start_key: start.map(str::to_owned),
            direction,
            page_size,
        }
    }

    fn run<S: UserStore>(
        store: &mut S,
        start: Option<&str>,
        direction: Direction,
        page_size: u16,
    ) -> Result<UserListPage, DomainError> {
        block_on(list(store, input(start, direction, page_size)))
    }

    fn ids(page: &UserListPage) -> Vec<String> {
        page.items
            .iter()
            .map(|item| item.sec_usr_id.trimmed().into_owned())
            .collect()
    }

    fn text(key: Option<UserId>) -> Option<String> {
        key.map(|id| id.trimmed().into_owned())
    }

    fn owned(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|&key| key.to_owned()).collect()
    }

    /// One request and the page the audit row expects for it.
    struct Case {
        row: &'static str,
        users: &'static [&'static str],
        start: Option<&'static str>,
        direction: Direction,
        page_size: u16,
        items: &'static [&'static str],
        next: Option<&'static str>,
        prev: Option<&'static str>,
        message: Option<&'static str>,
    }

    const CASES: [Case; 18] = [
        Case {
            row: "P-COUSR00C-003 full page, look-ahead at the end of the file",
            users: &SEEDED,
            start: None,
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &SEEDED,
            next: None,
            prev: Some("ADMIN001"),
            message: Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE),
        },
        Case {
            row: "P-COUSR00C-003 look-ahead user is the next key",
            users: &SEEDED,
            start: None,
            direction: Forward,
            page_size: 1,
            items: &["ADMIN001"],
            next: Some("ADMIN002"),
            prev: Some("ADMIN001"),
            message: None,
        },
        Case {
            row: "P-COUSR00C-003 STARTBR GTEQ from a key no user has",
            users: &THIRTEEN,
            start: Some("B"),
            direction: Forward,
            page_size: 3,
            items: &["BUSER001", "USER0001", "USER0002"],
            next: Some("USER0003"),
            prev: Some("BUSER001"),
            message: None,
        },
        Case {
            row: "P-COUSR00C-003 USRIDIN of spaces is LOW-VALUES",
            users: &SEEDED,
            start: Some("        "),
            direction: Forward,
            page_size: 2,
            items: &["ADMIN001", "ADMIN002"],
            next: Some("ADMIN003"),
            prev: Some("ADMIN001"),
            message: None,
        },
        Case {
            row: "P-COUSR00C-003 USRIDIN of low-values",
            users: &SEEDED,
            start: Some("\0\0\0\0\0\0\0\0"),
            direction: Forward,
            page_size: 2,
            items: &["ADMIN001", "ADMIN002"],
            next: Some("ADMIN003"),
            prev: Some("ADMIN001"),
            message: None,
        },
        Case {
            row: "V-COUSR00C-002",
            users: &SEEDED,
            start: Some("USER0005"),
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &["USER0005"],
            next: None,
            prev: Some("USER0005"),
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-003 forward",
            users: &SEEDED,
            start: Some("ZZZZZZZZ"),
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-003 backward",
            users: &SEEDED,
            start: Some("ZZZZZZZZ"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-003 lower case sorts after every seeded ID",
            users: &SEEDED,
            start: Some("user0001"),
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-003 empty file, forward",
            users: &[],
            start: None,
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-003 empty file, backward",
            users: &[],
            start: None,
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-005",
            users: &SEEDED,
            start: Some("USER0001"),
            direction: Forward,
            page_size: SCREEN_ROWS,
            items: &["USER0001", "USER0002", "USER0003", "USER0004", "USER0005"],
            next: None,
            prev: Some("USER0001"),
            message: Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-007 from the first user ID",
            users: &SEEDED,
            start: Some("ADMIN001"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-007 blank start key is LOW-VALUES",
            users: &SEEDED,
            start: None,
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-007 STARTBR positions at the first user ID",
            users: &SEEDED,
            start: Some("AAAAAAAA"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[],
            next: None,
            prev: None,
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "V-COUSR00C-008",
            users: &SEEDED,
            start: Some("ADMIN003"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &["ADMIN001", "ADMIN002"],
            next: Some("ADMIN003"),
            prev: Some("ADMIN001"),
            message: Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
        },
        Case {
            row: "P-COUSR00C-009 full page, look-ahead found",
            users: &THIRTEEN,
            start: Some("ZUSER001"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[
                "ADMIN003", "ADMIN004", "ADMIN005", "BUSER001", "USER0001", "USER0002", "USER0003",
                "USER0004", "USER0005", "USER0006",
            ],
            next: Some("ZUSER001"),
            prev: Some("ADMIN003"),
            message: None,
        },
        Case {
            row: "P-COUSR00C-009 STARTBR GTEQ from a key no user has",
            users: &THIRTEEN,
            start: Some("USER0000"),
            direction: Backward,
            page_size: SCREEN_ROWS,
            items: &[
                "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "BUSER001",
            ],
            next: Some("USER0001"),
            prev: Some("ADMIN001"),
            message: Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
        },
    ];

    #[test]
    fn pages_follow_the_audited_rows() {
        for case in &CASES {
            let mut store = store(case.users);
            let page = run(&mut store, case.start, case.direction, case.page_size)
                .unwrap_or_else(|error| panic!("{}: {error:?}", case.row));
            assert_eq!(ids(&page), owned(case.items), "{}", case.row);
            assert_eq!(
                text(page.next_key),
                case.next.map(str::to_owned),
                "{}",
                case.row
            );
            assert_eq!(
                text(page.prev_key),
                case.prev.map(str::to_owned),
                "{}",
                case.row
            );
            assert_eq!(page.message, case.message, "{}", case.row);
        }
    }

    #[test]
    fn ten_seeded_users_and_three_more_page_forward_and_back() {
        let mut store = store(&THIRTEEN);

        // P-COUSR00C-003, P-COUSR00C-010: the first ten users, each the
        // stored record, and the look-ahead user as the next key.
        let first = run(&mut store, None, Forward, SCREEN_ROWS).expect("page 1");
        assert_eq!(ids(&first), owned(&THIRTEEN[..10]));
        assert_eq!(first.items[0], user("ADMIN001"));
        assert_eq!(text(first.next_key).as_deref(), Some("USER0005"));
        assert_eq!(first.message, None);

        // PF8: the next key starts the rest without repeating a row, and the
        // end of the file is reached (V-COUSR00C-005).
        let next = first.next_key.map(|key| key.trimmed().into_owned());
        let second = run(&mut store, next.as_deref(), Forward, SCREEN_ROWS).expect("page 2");
        assert_eq!(ids(&second), owned(&THIRTEEN[10..]));
        assert_eq!(second.next_key, None);
        assert_eq!(
            second.message,
            Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE)
        );
        let mut all = ids(&first);
        all.extend(ids(&second));
        assert_eq!(all, owned(&THIRTEEN));

        // A further PF8, from the last user ID (V-COUSR00C-002).
        let further = run(&mut store, Some("ZUSER001"), Forward, SCREEN_ROWS).expect("PF8");
        assert_eq!(
            further.message,
            Some(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE)
        );
        assert_eq!(further.next_key, None);

        // PF7 from the first key of page 2 shows page 1 again, its look-ahead
        // meeting the start of the file (P-COUSR00C-009, V-COUSR00C-008).
        let prev = second.prev_key.map(|key| key.trimmed().into_owned());
        let back = run(&mut store, prev.as_deref(), Backward, SCREEN_ROWS).expect("PF7");
        assert_eq!(back.items, first.items);
        assert_eq!(back.next_key, first.next_key);
        assert_eq!(back.prev_key, first.prev_key);
        assert_eq!(
            back.message,
            Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE)
        );

        // PF7 again, from the first user ID (V-COUSR00C-007).
        let top = run(&mut store, Some("ADMIN001"), Backward, SCREEN_ROWS).expect("PF7 again");
        assert!(top.items.is_empty());
        assert_eq!(
            top.message,
            Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE)
        );
    }

    #[test]
    fn a_final_page_of_one_user_is_still_shown() {
        let mut store = store(&THIRTEEN[..11]);
        let first = run(&mut store, None, Forward, SCREEN_ROWS).expect("page 1");
        let next = first.next_key.map(|key| key.trimmed().into_owned());
        assert_eq!(next.as_deref(), Some("USER0005"));

        let last = run(&mut store, next.as_deref(), Forward, SCREEN_ROWS).expect("page 2");
        assert_eq!(ids(&last), ["USER0005"]);
        assert_eq!(
            last.message,
            Some(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE),
            "V-COUSR00C-002 ranks first for a forward request from the last user ID"
        );
    }

    /// A [`FakeStore`] whose backward browse fails: a `READPREV` failure
    /// after a good `STARTBR`.
    struct FailingReadPrev(FakeStore);

    impl UserStore for FailingReadPrev {
        async fn read_user(&mut self, id: UserId) -> PortResult<UserSecurity> {
            self.0.read_user(id).await
        }

        async fn read_user_for_update(&mut self, id: UserId) -> PortResult<UserSecurity> {
            self.0.read_user_for_update(id).await
        }

        async fn browse_users(&mut self, browse: Browse<UserId>) -> PortResult<Vec<UserSecurity>> {
            if browse.direction == Backward {
                return Err(PortError::Unavailable("READPREV failed".to_owned()));
            }
            self.0.browse_users(browse).await
        }

        async fn insert_user(&mut self, user: UserSecurity) -> PortResult<()> {
            self.0.insert_user(user).await
        }

        async fn update_user(&mut self, user: UserSecurity) -> PortResult<()> {
            self.0.update_user(user).await
        }

        async fn delete_user(&mut self, id: UserId) -> PortResult<()> {
            self.0.delete_user(id).await
        }
    }

    fn assert_lookup_failed(result: Result<UserListPage, DomainError>, row: &str) {
        let error = result.expect_err(row);
        assert_eq!(error.kind, ErrorKind::Unexpected, "{row}");
        assert_eq!(error.message, messages::UNABLE_TO_LOOKUP_USER, "{row}");
        assert_eq!(error.first_field(), Some(USRIDIN), "{row}");
        assert_eq!(
            error.fields[0].message,
            messages::UNABLE_TO_LOOKUP_USER,
            "{row}"
        );
    }

    #[test]
    fn browse_failures_are_the_lookup_error_on_usridin() {
        for (row, direction) in [("V-COUSR00C-006", Forward), ("V-COUSR00C-004", Backward)] {
            let mut store = store(&SEEDED);
            store.fail_next("browse_users", PortError::Unavailable("down".to_owned()));
            assert_lookup_failed(run(&mut store, None, direction, SCREEN_ROWS), row);
            assert_eq!(
                store.calls,
                ["browse_users"],
                "{row}: nothing read after it"
            );
        }

        let mut store = FailingReadPrev(store(&SEEDED));
        let result = run(&mut store, Some("ADMIN003"), Backward, SCREEN_ROWS);
        assert_lookup_failed(result, "V-COUSR00C-009");
        assert_eq!(store.0.calls, ["browse_users"]);
    }

    #[test]
    fn the_list_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = store(&SEEDED);
        let future = list(&mut store, input(None, Forward, SCREEN_ROWS));
        assert_send(&future);
        assert_eq!(ids(&block_on(future).expect("a page")), owned(&SEEDED));
    }
}
