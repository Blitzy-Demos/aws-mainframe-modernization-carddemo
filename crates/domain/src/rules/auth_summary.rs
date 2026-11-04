//! `COPAUS0C` (CPVS): the pending authorizations of one account, the
//! `GET /authorizations?account_id=&start_key=&direction=&page_size=` resource.
//!
//! [`summary`] ports the account edit of `PROCESS-ENTER-KEY`, the reads of
//! `GATHER-ACCOUNT-DETAILS` (card cross-reference by account, account,
//! customer, then the IMS summary segment `PAUTSUM0`), and the `PAUTDTL1` page
//! that `PROCESS-PAGE-FORWARD` fills for ENTER, PF7 and PF8
//! [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L261-1000]. The program
//! reads and writes nothing else, so the rule only reads (P-COPAUS0C-015).
//!
//! # Paging without a COMMAREA
//!
//! The COBOL keeps its page state in the COMMAREA: the last key shown
//! (`CDEMO-CPVS-PAUKEY-LAST`), the first key of each page shown
//! (`CDEMO-CPVS-PAUKEY-PREV-PG`), the page number and the next-page flag. The
//! stateless request carries a key instead (E-54):
//!
//! - no `start_key`, forward: ENTER, the first page from the newest
//!   authorization (P-COPAUS0C-010);
//! - `start_key` forward: PF8 from the last authorization shown, which the
//!   COBOL repositions on and discards before it reads on (`next_key`). A
//!   request from the last authorization of the account browses nothing and
//!   answers `You are already at the bottom of the page...` (P-COPAUS0C-012,
//!   V-COPAUS0C-013);
//! - `start_key` backward: PF7 from the first authorization shown, which
//!   shows the page before it (`prev_key`). A request from the newest
//!   authorization browses nothing and answers
//!   `You are already at the top of the page...` (P-COPAUS0C-011,
//!   V-COPAUS0C-014);
//! - no `start_key`, backward: the last page, the oldest authorizations. No
//!   COBOL key reaches it; it is the end-of-file start of the browse contract.
//!
//! Every page lists its authorizations in IMS twin order, newest first
//! (P-COPAUS0C-010). With one row per page this is also the next and previous
//! authorization of the detail screen (E-59).
//!
//! # Not reproduced
//!
//! - The row selectors and `Invalid selection. Valid value is S`
//!   (P-COPAUS0C-004, P-COPAUS0C-005, V-COPAUS0C-003): no request parameter
//!   carries a selector; each listed row links to its detail instead (E-54).
//! - Reads after a failed one: `GATHER-ACCOUNT-DETAILS` performs all its reads
//!   whatever the earlier ones found, so a later message can overwrite an
//!   earlier one. The request stops at the first failure and returns its
//!   message (E-50). For an account without a cross-reference the COBOL's
//!   final text cannot be pinned (E-52, halt), and the rule returns the
//!   cross-reference text the escalation proposes, with no list.
//! - The PSB schedule (P-COPAUS0C-020): no port opens the authorization store
//!   separately, so [`summary`] never fails with its text; the adapter that
//!   opens the store reports that failure with [`schedule_psb_failure`].
//!
//! The CICS `RESP` and IMS status codes inside the messages are those E-44
//! proposes for `PostgreSQL` outcomes.

use crate::entities::{Account, CardXref, Customer, PendingAuthDetail};
use crate::error::DomainError;
use crate::messages::{self, AuthLookupFailure};
use crate::numeric::{AccountId, AuthId, CustomerId};
use crate::ports::{
    AccountReader, AuthorizationStore, Browse, CustomerReader, Direction, PortError,
};

/// The summary record and the cross-reference reader [`summary`] works on,
/// re-exported from [`crate::entities`] and [`crate::ports`] for the API
/// adapter.
pub use crate::entities::PendingAuthSummary;
pub use crate::ports::XrefReader;

/// The rows of one page: the five list lines `SEL0001` to `SEL0005` of
/// `COPAU00` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU00.cpy:L150], filled
/// `UNTIL WS-IDX > 5` [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L424]
/// — P-COPAUS0C-010.
pub const SCREEN_ROWS: u16 = 5;

/// The BMS field every `COPAUS0C` message places the cursor on,
/// `MOVE -1 TO ACCTIDL OF COPAU0AI` — V-COPAUS0C-001 to V-COPAUS0C-015.
const ACCTID: &str = "ACCTID";

/// `ACCTIDI PIC X(11)` [app/app-authorization-ims-db2-mq/cpy-bms/COPAU00.cpy:L60]:
/// the class test `NUMERIC` passes only when all eleven bytes are digits.
const ACCOUNT_ID_LEN: usize = 11;

/// The CICS `RESP` and `RESP2` of `READ` `NOTFND` (E-44).
const RESP_NOTFND: u32 = 13;
const RESP2_NOTFND: u32 = 80;

/// The `RESP` (`IOERR`) and `RESP2` E-44 proposes for any other read failure.
const RESP_OTHER: u32 = 17;
const RESP2_OTHER: u32 = 0;

/// The IMS status E-44 proposes for any failed authorization-store access.
const IMS_STATUS_OTHER: &str = "AO";

/// One `GET /authorizations` request: the received `ACCTIDI` and the browse
/// position the COMMAREA held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSummaryInput {
    /// The account ID as entered, unedited; `None` when the request has none.
    pub account_id: Option<String>,
    /// The authorization the browse continues from: the last one shown for a
    /// forward request (PF8), the first one shown for a backward request
    /// (PF7), `None` for the first page (ENTER).
    pub start_key: Option<AuthId>,
    /// Forward (ENTER, PF8) or backward (PF7).
    pub direction: Direction,
    /// The most authorizations listed; [`SCREEN_ROWS`] on the 3270 screen.
    pub page_size: u16,
}

/// The `COPAU00` screen as one page: the header records, the summary
/// counters and one page of authorizations.
///
/// Each listed authorization shows `A` when [`PendingAuthDetail::is_approved`]
/// holds and `D` otherwise (`POPULATE-AUTH-LIST`, P-COPAUS0C-013); its date,
/// time and amount editing belong to the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSummaryView {
    /// `ACCTDAT` record: credit and cash limits (P-COPAUS0C-006).
    pub account: Account,
    /// `CUSTDAT` record: name, address and phone (P-COPAUS0C-006).
    pub customer: Customer,
    /// The account's first `CXACAIX` cross-reference (P-COPAUS0C-007).
    pub card_xref: CardXref,
    /// The `PAUTSUM0` segment; `None` when the account has none, which the
    /// COBOL shows as zero counts and amounts (P-COPAUS0C-006,
    /// P-COPAUS0C-009).
    pub summary: Option<PendingAuthSummary>,
    /// The page of `PAUTDTL1` details in twin order, newest first.
    pub details: Vec<PendingAuthDetail>,
    /// The last authorization listed when a later one exists: the
    /// `start_key` of the forward request for the next page.
    pub next_key: Option<AuthId>,
    /// The first authorization listed when an earlier one exists: the
    /// `start_key` of the backward request for the previous page.
    pub prev_key: Option<AuthId>,
    /// The page-boundary text of a request that browsed nothing.
    pub message: Option<&'static str>,
}

/// One page of details with its keys and boundary text.
#[derive(Debug, Default)]
struct DetailPage {
    details: Vec<PendingAuthDetail>,
    next_key: Option<AuthId>,
    prev_key: Option<AuthId>,
    message: Option<&'static str>,
}

impl DetailPage {
    /// A request that browses nothing and shows `message`.
    fn boundary(message: &'static str) -> Self {
        Self {
            message: Some(message),
            ..Self::default()
        }
    }
}

/// COPAUS0C PROCESS-ENTER-KEY — P-COPAUS0C-003, V-COPAUS0C-001, V-COPAUS0C-002
///
/// The account ID edit: `ACCTIDI = SPACES OR LOW-VALUES`, then
/// `ACCTIDI IS NOT NUMERIC` on the eleven bytes of the field. Absent or empty
/// text, and text of spaces only or of NUL characters only, is blank; any
/// other text that is not exactly eleven ASCII digits fails the class test, as
/// the space-padded field of a shorter entry does. The edit reads no store, so
/// an adapter runs it before it opens one, and its errors precede the
/// scheduling failure of `SCHEDULE-PSB` (V-COPAUS0C-010).
///
/// # Errors
///
/// [`ErrorKind::Empty`](crate::error::ErrorKind::Empty) with
/// `Please enter Acct Id...` for a blank entry, and
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with
/// `Acct Id must be Numeric ...` for a non-numeric one, both on `ACCTID`.
pub fn edit_account_id(raw: Option<&str>) -> Result<AccountId, DomainError> {
    let text = raw.unwrap_or_default();
    if text.bytes().all(|byte| byte == b' ') || text.bytes().all(|byte| byte == 0) {
        return Err(DomainError::empty(messages::PLEASE_ENTER_ACCT_ID).with_field(ACCTID));
    }
    let not_numeric = || DomainError::invalid(messages::ACCT_ID_MUST_BE_NUMERIC).with_field(ACCTID);
    if text.len() != ACCOUNT_ID_LEN || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(not_numeric());
    }
    text.parse::<AccountId>().map_err(|_| not_numeric())
}

/// COPAUS0C SCHEDULE-PSB — P-COPAUS0C-020, P-COPAUS0C-021, V-COPAUS0C-010
///
/// The error of a request whose authorization store cannot be opened: the
/// COBOL shows ` System error while scheduling PSB: Code:` with the status,
/// then abends on its next DL/I call; the replacement answers with that text
/// and does not abend (E-71). [`summary`] never returns it, because no port
/// schedules a PSB.
#[must_use]
pub fn schedule_psb_failure() -> DomainError {
    ims_error(messages::SYSTEM_ERROR_WHILE_SCHEDULING_PSB)
}

/// COPAUS0C GATHER-DETAILS — P-COPAUS0C-001, P-COPAUS0C-003, P-COPAUS0C-006,
/// P-COPAUS0C-010 to P-COPAUS0C-012
///
/// Edits the account ID, reads the cross-reference, account, customer and
/// summary of the account, and, when the summary exists, the page of details
/// that [`AuthSummaryInput::start_key`] and [`AuthSummaryInput::direction`]
/// select, as the module documentation describes. The rule writes nothing.
///
/// # Errors
///
/// The [`edit_account_id`] errors; then, on `ACCTID`, the first read that
/// fails: [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) when the
/// account has no cross-reference (V-COPAUS0C-004) or the cross-reference
/// names no account (V-COPAUS0C-006) or no customer (V-COPAUS0C-008), and
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the
/// system-error text of the file or segment for any other failure
/// (V-COPAUS0C-005, V-COPAUS0C-007, V-COPAUS0C-009, V-COPAUS0C-011,
/// V-COPAUS0C-012, V-COPAUS0C-015).
pub async fn summary<S>(
    store: &mut S,
    input: AuthSummaryInput,
) -> Result<AuthSummaryView, DomainError>
where
    S: XrefReader + AccountReader + CustomerReader + AuthorizationStore,
{
    let account_id = edit_account_id(input.account_id.as_deref())?;
    let card_xref = read_xref(store, account_id).await?;
    let account = read_account(store, card_xref.xref_acct_id).await?;
    let customer = read_customer(store, card_xref.xref_cust_id).await?;
    let segment = read_summary(store, account_id).await?;
    let page = if segment.is_some() {
        browse_details(store, account_id, &input).await?
    } else {
        DetailPage::default()
    };
    Ok(AuthSummaryView {
        account,
        customer,
        card_xref,
        summary: segment,
        details: page.details,
        next_key: page.next_key,
        prev_key: page.prev_key,
        message: page.message,
    })
}

/// COPAUS0C GETCARDXREF-BYACCT — P-COPAUS0C-007, V-COPAUS0C-004,
/// V-COPAUS0C-005
///
/// `READ` of `CXACAIX` by the entered account, which the `NOTFND` text names
/// as `WS-ACCT-ID` and the other-failure text as `WS-CARD-RID-ACCT-ID-X`, the
/// same eleven digits.
///
/// On `NOTFND` the COBOL sets no error flag and reads on with the keys the
/// failed read left, a result that cannot be pinned (E-52, halt). The request
/// stops here with the cross-reference text and lists nothing, the outcome
/// the escalation proposes, rather than guessing the later reads.
async fn read_xref<S: XrefReader>(
    store: &mut S,
    account_id: AccountId,
) -> Result<CardXref, DomainError> {
    store
        .read_xref_by_account(account_id)
        .await
        .map_err(|error| {
            lookup_error(
                &error,
                [
                    AuthLookupFailure::XrefNotFound,
                    AuthLookupFailure::XrefReadError,
                ],
                &account_id.to_string(),
            )
        })
}

/// COPAUS0C GETACCTDATA-BYACCT — P-COPAUS0C-008, V-COPAUS0C-006,
/// V-COPAUS0C-007
///
/// `READ` of `ACCTDAT` by `XREF-ACCT-ID`, which the messages name.
async fn read_account<S: AccountReader>(
    store: &mut S,
    account_id: AccountId,
) -> Result<Account, DomainError> {
    store.read_account(account_id).await.map_err(|error| {
        lookup_error(
            &error,
            [
                AuthLookupFailure::AccountNotFound,
                AuthLookupFailure::AccountReadError,
            ],
            &account_id.to_string(),
        )
    })
}

/// COPAUS0C GETCUSTDATA-BYCUST — P-COPAUS0C-008, V-COPAUS0C-008,
/// V-COPAUS0C-009
///
/// `READ` of `CUSTDAT` by `XREF-CUST-ID`, which the messages name in its nine
/// digits.
async fn read_customer<S: CustomerReader>(
    store: &mut S,
    customer_id: CustomerId,
) -> Result<Customer, DomainError> {
    store.read_customer(customer_id).await.map_err(|error| {
        lookup_error(
            &error,
            [
                AuthLookupFailure::CustomerNotFound,
                AuthLookupFailure::CustomerReadError,
            ],
            &customer_id.to_string(),
        )
    })
}

/// COPAUS0C GET-AUTH-SUMMARY — P-COPAUS0C-009, V-COPAUS0C-011
///
/// `GU PAUTSUM0 WHERE (ACCNTID = CDEMO-ACCT-ID)`: status `GE` is no summary
/// and no message; any other failure is the summary system error.
async fn read_summary<S: AuthorizationStore>(
    store: &mut S,
    account_id: AccountId,
) -> Result<Option<PendingAuthSummary>, DomainError> {
    match store.read_summary(account_id).await {
        Ok(segment) => Ok(Some(segment)),
        Err(PortError::NotFound) => Ok(None),
        Err(_) => Err(ims_error(
            messages::SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS0C,
        )),
    }
}

/// COPAUS0C PROCESS-PAGE-FORWARD, GET-AUTHORIZATIONS, REPOSITION-AUTHORIZATIONS
/// — P-COPAUS0C-010 to P-COPAUS0C-012, P-COPAUS0C-014, V-COPAUS0C-012 to
/// V-COPAUS0C-015
///
/// One browse of the account's twins: the page, one look-ahead row that
/// decides whether a page follows, and, for a keyed request, the record of
/// the start key itself, on which the COBOL repositions before it reads on.
///
/// A keyed request's first DL/I call is that repositioning `GNP … WHERE
/// (PAUT9CTS = key)`, so its failure shows the repositioning text; the first
/// page's is the unqualified `GNP`, which shows the reading text. One port
/// call serves both COBOL calls, so the text follows the first call the
/// COBOL path makes.
async fn browse_details<S: AuthorizationStore>(
    store: &mut S,
    account_id: AccountId,
    input: &AuthSummaryInput,
) -> Result<DetailPage, DomainError> {
    let rows = usize::from(input.page_size);
    let (extra, failure) = match input.start_key {
        Some(_) => (2, messages::SYSTEM_ERROR_WHILE_REPOS_AUTH_DETAILS),
        None => (
            1,
            messages::SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS0C,
        ),
    };
    let browse = Browse {
        start_key: input.start_key,
        direction: input.direction,
        limit: input.page_size.saturating_add(extra),
    };
    let twins = store
        .details_for_account(account_id, browse)
        .await
        .map_err(|_| ims_error(failure))?;
    Ok(match (input.start_key, input.direction) {
        (None, Direction::Forward) => first_page(twins, rows),
        (None, Direction::Backward) => last_page(twins, rows),
        (Some(_), Direction::Forward) => page_after_key(twins, rows),
        (Some(_), Direction::Backward) => page_before_key(twins, rows),
    })
}

/// COPAUS0C PROCESS-PAGE-FORWARD — P-COPAUS0C-010
///
/// The first `rows` of `twins`, read in twin order; the look-ahead row past
/// them makes the last one listed the next page's key (`NEXT-PAGE-YES`).
fn first_page(mut twins: Vec<PendingAuthDetail>, rows: usize) -> DetailPage {
    let more = twins.len() > rows;
    twins.truncate(rows);
    let next_key = more.then(|| last_key(&twins)).flatten();
    DetailPage {
        details: twins,
        next_key,
        ..DetailPage::default()
    }
}

/// A page from `twins` read backward, from the end of the twins or from
/// below a start key: the first `rows` of them, put back in twin order; the
/// look-ahead row past them makes the first one listed the previous page's
/// key.
fn last_page(twins: Vec<PendingAuthDetail>, rows: usize) -> DetailPage {
    let more = twins.len() > rows;
    let mut details: Vec<PendingAuthDetail> = twins.into_iter().take(rows).collect();
    details.reverse();
    let prev_key = more.then(|| first_key(&details)).flatten();
    DetailPage {
        details,
        prev_key,
        ..DetailPage::default()
    }
}

/// COPAUS0C PROCESS-PF8-KEY — P-COPAUS0C-010, P-COPAUS0C-012, V-COPAUS0C-013
///
/// `twins` read forward from the start key, whose own record comes first and
/// is discarded, as the COBOL discards the record it repositions on. A start
/// key with no twin after it is the last page's last key: nothing is browsed
/// and the bottom-of-page text is shown. A start key that names no twin of
/// the account finds nothing (status `GE`) and shows no text.
fn page_after_key(twins: Vec<PendingAuthDetail>, rows: usize) -> DetailPage {
    match twins.len() {
        0 => DetailPage::default(),
        1 => DetailPage::boundary(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE),
        _ => {
            let mut page = first_page(twins.into_iter().skip(1).collect(), rows);
            page.prev_key = first_key(&page.details);
            page
        }
    }
}

/// COPAUS0C PROCESS-PF7-KEY — P-COPAUS0C-011, V-COPAUS0C-014
///
/// `twins` read backward from the start key, the first authorization shown,
/// whose own record comes first and is not listed: the page holds the `rows`
/// twins before it, in twin order. A start key with no twin before it is the
/// first page's first key: nothing is browsed and the top-of-page text is
/// shown. A start key that names no twin of the account finds nothing and
/// shows no text.
fn page_before_key(twins: Vec<PendingAuthDetail>, rows: usize) -> DetailPage {
    match twins.len() {
        0 => DetailPage::default(),
        1 => DetailPage::boundary(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
        _ => {
            let mut page = last_page(twins.into_iter().skip(1).collect(), rows);
            page.next_key = last_key(&page.details);
            page
        }
    }
}

/// The identity of the first detail listed.
fn first_key(details: &[PendingAuthDetail]) -> Option<AuthId> {
    details.first().map(|detail| detail.auth_id)
}

/// The identity of the last detail listed.
fn last_key(details: &[PendingAuthDetail]) -> Option<AuthId> {
    details.last().map(|detail| detail.auth_id)
}

/// The `COPAUS0C` message of a failed cross-reference, account or customer
/// read: `failures[0]` with the `NOTFND` codes for a missing record,
/// `failures[1]` with the other-failure codes for anything else (the
/// `WHEN OTHER` branch), both on `ACCTID` (E-44, E-50).
fn lookup_error(error: &PortError, failures: [AuthLookupFailure; 2], key: &str) -> DomainError {
    let [not_found, other] = failures;
    if matches!(error, PortError::NotFound) {
        let message = messages::auth_lookup_message(not_found, key, RESP_NOTFND, RESP2_NOTFND);
        DomainError::not_found(message).with_field(ACCTID)
    } else {
        let message = messages::auth_lookup_message(other, key, RESP_OTHER, RESP2_OTHER);
        DomainError::unexpected(message).with_field(ACCTID)
    }
}

/// A failed authorization-store access: the IMS system-error `head` with the
/// status E-44 proposes, on `ACCTID`.
fn ims_error(head: &str) -> DomainError {
    DomainError::unexpected(messages::ims_status_message(head, IMS_STATUS_OTHER)).with_field(ACCTID)
}

#[cfg(test)]
mod tests {
    use super::{
        AuthSummaryInput, AuthSummaryView, SCREEN_ROWS, edit_account_id, schedule_psb_failure,
        summary,
    };
    use crate::entities::{
        Account, CardXref, Customer, FraudFlag, MatchStatus, NewPendingAuthDetail,
        PA_ACCOUNT_STATUS_OCCURS, PendingAuthDetail, PendingAuthSummary,
    };
    use crate::error::{DomainError, ErrorKind};
    use crate::messages::{self, AuthLookupFailure};
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, AuthCount, AuthId, CardNumber, CustSsn, CustomerId,
        FicoScore, Money, NumericCode,
    };
    use crate::ports::{Direction, PortError};
    use crate::rules::test_support::{FakeStore, block_on};

    /// The account every fixture lists, and its customer.
    const ACCOUNT: u64 = 1;
    const CUSTOMER: u32 = 7;
    const ACCOUNT_TEXT: &str = "00000000001";
    const CUSTOMER_TEXT: &str = "000000007";
    const CARD: &str = "4111000000000001";

    /// The `PA-AUTH-TIME-9C` of the seven details, in insert order, so their
    /// identities are 1 to 7. Ascending key is twin order, newest first.
    const TIMES: [i32; 7] = [700, 100, 300, 200, 600, 400, 500];
    /// The identities of the seven details in twin order.
    const TWINS: [i64; 7] = [2, 4, 3, 6, 7, 5, 1];

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn account() -> Account {
        Account {
            acct_id: account_id(ACCOUNT),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(19_400),
            acct_credit_limit: cents(2_020_000),
            acct_cash_credit_limit: cents(1_020_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn customer() -> Customer {
        Customer {
            cust_id: CustomerId::new(CUSTOMER).expect("a nine-digit customer"),
            cust_first_name: Alnum::move_from("Immanuel"),
            cust_middle_name: Alnum::move_from("Madeline"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
    }

    fn xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(CARD),
            xref_cust_id: CustomerId::new(CUSTOMER).expect("a nine-digit customer"),
            xref_acct_id: account_id(ACCOUNT),
        }
    }

    fn pending_summary() -> PendingAuthSummary {
        PendingAuthSummary {
            pa_acct_id: account_id(ACCOUNT),
            pa_cust_id: CustomerId::new(CUSTOMER).expect("a nine-digit customer"),
            pa_auth_status: Alnum::move_from("A"),
            pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
            pa_credit_limit: cents(2_020_000),
            pa_cash_limit: cents(1_020_000),
            pa_credit_balance: cents(12_000),
            pa_cash_balance: Money::ZERO,
            pa_approved_auth_cnt: AuthCount::new(6).expect("a count"),
            pa_declined_auth_cnt: AuthCount::new(1).expect("a count"),
            pa_approved_auth_amt: cents(12_000),
            pa_declined_auth_amt: cents(75_000),
        }
    }

    /// A detail of the account keyed `79_797`/`time_9c`; response `05`
    /// (declined) when `resp_code` says so, else `00`.
    fn detail(time_9c: i32, resp_code: &str) -> NewPendingAuthDetail {
        NewPendingAuthDetail {
            pa_acct_id: account_id(ACCOUNT),
            pa_auth_date_9c: 79_797,
            pa_auth_time_9c: time_9c,
            pa_auth_orig_date: Alnum::move_from("220718"),
            pa_auth_orig_time: Alnum::move_from("101530"),
            pa_card_num: CardNumber::move_from(CARD),
            pa_auth_type: Alnum::move_from("0100"),
            pa_card_expiry_date: Alnum::move_from("0525"),
            pa_message_type: Alnum::move_from("0110"),
            pa_message_source: Alnum::move_from("POS"),
            pa_auth_id_code: Alnum::move_from("101530"),
            pa_auth_resp_code: Alnum::move_from(resp_code),
            pa_auth_resp_reason: Alnum::move_from("0000"),
            pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
            pa_transaction_amt: cents(2_000),
            pa_approved_amt: cents(2_000),
            pa_merchant_catagory_code: Alnum::move_from("5411"),
            pa_acqr_country_code: Alnum::move_from("840"),
            pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
            pa_merchant_id: Alnum::move_from("800000000"),
            pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
            pa_merchant_city: Alnum::move_from("Enoshaven"),
            pa_merchant_state: Alnum::move_from("NC"),
            pa_merchant_zip: Alnum::move_from("72112"),
            pa_transaction_id: Alnum::move_from("000000000000001"),
            pa_match_status: MatchStatus::Pending,
            pa_auth_fraud: FraudFlag::Blank,
            pa_fraud_rpt_date: Alnum::spaces(),
        }
    }

    /// The account, its customer and cross-reference, without a summary.
    fn header_only() -> FakeStore {
        FakeStore::default()
            .with_account(account())
            .with_customer(customer())
            .with_xref(xref())
    }

    /// The account with its summary and the seven details of [`TIMES`]; the
    /// detail keyed 200 (identity 4) is declined.
    fn seven_details() -> FakeStore {
        TIMES.iter().fold(
            header_only().with_summary(pending_summary()),
            |store, &time| {
                let resp_code = if time == 200 { "05" } else { "00" };
                store.with_detail(detail(time, resp_code))
            },
        )
    }

    fn input(start_key: Option<i64>, direction: Direction, page_size: u16) -> AuthSummaryInput {
        AuthSummaryInput {
            account_id: Some(ACCOUNT_TEXT.to_owned()),
            start_key: start_key.map(AuthId::from),
            direction,
            page_size,
        }
    }

    fn run(
        store: &mut FakeStore,
        request: AuthSummaryInput,
    ) -> Result<AuthSummaryView, DomainError> {
        block_on(summary(store, request))
    }

    fn listed(view: &AuthSummaryView) -> Vec<i64> {
        view.details
            .iter()
            .map(|detail| detail.auth_id.get())
            .collect()
    }

    fn key(value: Option<AuthId>) -> Option<i64> {
        value.map(AuthId::get)
    }

    /// The error a rule returns on `ACCTID` with `kind` and `message`.
    fn on_acctid(kind: ErrorKind, message: impl Into<String>) -> DomainError {
        DomainError::new(kind, message).with_field("ACCTID")
    }

    /// A failed lookup's error, as `COPAUS0C` composes it.
    fn lookup(
        kind: ErrorKind,
        failure: AuthLookupFailure,
        key: &str,
        codes: (u32, u32),
    ) -> DomainError {
        on_acctid(
            kind,
            messages::auth_lookup_message(failure, key, codes.0, codes.1),
        )
    }

    /// A failed authorization-store access's error.
    fn ims(head: &str) -> DomainError {
        on_acctid(
            ErrorKind::Unexpected,
            messages::ims_status_message(head, "AO"),
        )
    }

    #[test]
    fn edit_account_id_rejects_blank_then_non_numeric_input() {
        // V-COPAUS0C-001, V-COPAUS0C-002, P-COPAUS0C-003
        let blank = on_acctid(ErrorKind::Empty, messages::PLEASE_ENTER_ACCT_ID);
        let not_numeric = on_acctid(ErrorKind::Invalid, messages::ACCT_ID_MUST_BE_NUMERIC);
        let cases = [
            (None, Err(blank.clone())),
            (Some(""), Err(blank.clone())),
            (Some("           "), Err(blank.clone())),
            (Some("\0\0\0\0\0\0\0\0\0\0\0"), Err(blank)),
            (Some("0000000000A"), Err(not_numeric.clone())),
            (Some("1"), Err(not_numeric.clone())),
            (Some(" 0000000001"), Err(not_numeric.clone())),
            (Some("000000000012"), Err(not_numeric.clone())),
            (Some("+0000000001"), Err(not_numeric.clone())),
            (Some(" \0"), Err(not_numeric)),
            (Some("00000000001"), Ok(account_id(1))),
            (Some("00000000000"), Ok(account_id(0))),
            (Some("99999999999"), Ok(account_id(99_999_999_999))),
        ];
        for (raw, expected) in cases {
            assert_eq!(edit_account_id(raw), expected, "{raw:?}");
        }
    }

    #[test]
    fn a_rejected_account_id_reads_nothing() {
        // V-COPAUS0C-001, V-COPAUS0C-002: the edits run before any read.
        for raw in [None, Some("0000000000A")] {
            let mut store = seven_details();
            let request = AuthSummaryInput {
                account_id: raw.map(str::to_owned),
                ..input(None, Direction::Forward, SCREEN_ROWS)
            };
            assert!(run(&mut store, request).is_err(), "{raw:?}");
            assert!(store.calls.is_empty(), "{raw:?}");
        }
    }

    #[test]
    fn the_first_page_lists_the_five_newest_with_the_header() {
        // P-COPAUS0C-006, P-COPAUS0C-009, P-COPAUS0C-010, P-COPAUS0C-013
        let mut store = seven_details();
        let view =
            run(&mut store, input(None, Direction::Forward, SCREEN_ROWS)).expect("the first page");

        assert_eq!(SCREEN_ROWS, 5);
        assert_eq!(view.account, account());
        assert_eq!(view.customer, customer());
        assert_eq!(view.card_xref, xref());
        assert_eq!(view.summary, Some(pending_summary()));
        assert_eq!(listed(&view), TWINS[..5].to_vec());
        assert_eq!(key(view.next_key), Some(7));
        assert_eq!(key(view.prev_key), None);
        assert_eq!(view.message, None);
        let approved: Vec<bool> = view
            .details
            .iter()
            .map(PendingAuthDetail::is_approved)
            .collect();
        assert_eq!(approved, [true, false, true, true, true]);
        assert_eq!(
            store.calls,
            [
                "read_xref_by_account",
                "read_account",
                "read_customer",
                "read_summary",
                "details_for_account",
            ]
        );
    }

    /// One keyed or unkeyed page request and the page it gives.
    struct PageCase {
        start_key: Option<i64>,
        direction: Direction,
        page_size: u16,
        listed: &'static [i64],
        next_key: Option<i64>,
        prev_key: Option<i64>,
        message: Option<&'static str>,
    }

    const fn case(
        start_key: Option<i64>,
        direction: Direction,
        page_size: u16,
        listed: &'static [i64],
        keys: (Option<i64>, Option<i64>),
        message: Option<&'static str>,
    ) -> PageCase {
        PageCase {
            start_key,
            direction,
            page_size,
            listed,
            next_key: keys.0,
            prev_key: keys.1,
            message,
        }
    }

    #[test]
    fn pages_follow_their_keys_and_stop_at_the_top_and_bottom() {
        // P-COPAUS0C-010, P-COPAUS0C-011, P-COPAUS0C-012, V-COPAUS0C-013,
        // V-COPAUS0C-014; the one-row pages are the detail screen's Next and
        // Previous (E-59).
        use Direction::{Backward, Forward};
        let top = Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE);
        let bottom = Some(messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE);
        let cases = [
            case(Some(7), Forward, 5, &[5, 1], (None, Some(5)), None),
            case(Some(1), Forward, 5, &[], (None, None), bottom),
            case(
                Some(5),
                Backward,
                5,
                &[2, 4, 3, 6, 7],
                (Some(7), None),
                None,
            ),
            case(Some(2), Backward, 5, &[], (None, None), top),
            case(Some(4), Forward, 2, &[3, 6], (Some(6), Some(3)), None),
            case(Some(7), Backward, 2, &[3, 6], (Some(6), Some(3)), None),
            case(Some(6), Forward, 1, &[7], (Some(7), Some(7)), None),
            case(Some(6), Backward, 1, &[3], (Some(3), Some(3)), None),
            case(Some(1), Forward, 1, &[], (None, None), bottom),
            case(Some(2), Backward, 1, &[], (None, None), top),
            case(None, Backward, 5, &[3, 6, 7, 5, 1], (None, Some(3)), None),
            case(None, Backward, 7, &TWINS, (None, None), None),
            case(None, Forward, 7, &TWINS, (None, None), None),
            case(Some(99), Forward, 5, &[], (None, None), None),
            case(Some(99), Backward, 5, &[], (None, None), None),
        ];
        for case in cases {
            let label = (case.start_key, case.direction, case.page_size);
            let mut store = seven_details();
            let request = input(case.start_key, case.direction, case.page_size);
            let view = run(&mut store, request).expect("a page");
            assert_eq!(listed(&view), case.listed, "{label:?}");
            assert_eq!(key(view.next_key), case.next_key, "{label:?}");
            assert_eq!(key(view.prev_key), case.prev_key, "{label:?}");
            assert_eq!(view.message, case.message, "{label:?}");
            assert_eq!(view.summary, Some(pending_summary()), "{label:?}");
        }
    }

    #[test]
    fn an_account_without_a_summary_lists_nothing_and_shows_no_text() {
        // P-COPAUS0C-006, P-COPAUS0C-009, V-COPAUS0C-011 (status GE)
        for (start_key, direction) in [
            (None, Direction::Forward),
            (Some(1), Direction::Forward),
            (Some(1), Direction::Backward),
        ] {
            let mut store = header_only();
            let view = run(&mut store, input(start_key, direction, SCREEN_ROWS))
                .expect("a page without a summary");
            assert_eq!(view.summary, None);
            assert!(view.details.is_empty());
            assert_eq!(
                (view.next_key, view.prev_key, view.message),
                (None, None, None)
            );
            assert_eq!(
                store.calls,
                [
                    "read_xref_by_account",
                    "read_account",
                    "read_customer",
                    "read_summary"
                ]
            );
        }
    }

    #[test]
    fn an_account_without_a_cross_reference_stops_with_its_text() {
        // V-COPAUS0C-004, P-COPAUS0C-007; E-52: no read follows the NOTFND.
        let mut store = seven_details();
        let request = AuthSummaryInput {
            account_id: Some("99999999999".to_owned()),
            ..input(None, Direction::Forward, SCREEN_ROWS)
        };
        let expected = lookup(
            ErrorKind::NotFound,
            AuthLookupFailure::XrefNotFound,
            "99999999999",
            (13, 80),
        );
        assert_eq!(run(&mut store, request), Err(expected));
        assert_eq!(store.calls, ["read_xref_by_account"]);
    }

    #[test]
    fn read_failures_answer_the_audited_texts() {
        // V-COPAUS0C-005 to V-COPAUS0C-009, V-COPAUS0C-011, V-COPAUS0C-012,
        // V-COPAUS0C-015, P-COPAUS0C-008, P-COPAUS0C-014; codes per E-44.
        use AuthLookupFailure::{
            AccountNotFound, AccountReadError, CustomerNotFound, CustomerReadError, XrefReadError,
        };
        use Direction::{Backward, Forward};
        use ErrorKind::{NotFound, Unexpected};
        let down = || PortError::Unavailable("connection reset".to_owned());
        let reading = ims(messages::SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS0C);
        let repos = ims(messages::SYSTEM_ERROR_WHILE_REPOS_AUTH_DETAILS);
        let cases = [
            (
                "read_xref_by_account",
                down(),
                None,
                Forward,
                lookup(Unexpected, XrefReadError, ACCOUNT_TEXT, (17, 0)),
            ),
            (
                "read_xref_by_account",
                PortError::Contention,
                None,
                Forward,
                lookup(Unexpected, XrefReadError, ACCOUNT_TEXT, (17, 0)),
            ),
            (
                "read_account",
                PortError::NotFound,
                None,
                Forward,
                lookup(NotFound, AccountNotFound, ACCOUNT_TEXT, (13, 80)),
            ),
            (
                "read_account",
                down(),
                None,
                Forward,
                lookup(Unexpected, AccountReadError, ACCOUNT_TEXT, (17, 0)),
            ),
            (
                "read_customer",
                PortError::NotFound,
                None,
                Forward,
                lookup(NotFound, CustomerNotFound, CUSTOMER_TEXT, (13, 80)),
            ),
            (
                "read_customer",
                down(),
                None,
                Forward,
                lookup(Unexpected, CustomerReadError, CUSTOMER_TEXT, (17, 0)),
            ),
            (
                "read_summary",
                down(),
                None,
                Forward,
                ims(messages::SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS0C),
            ),
            (
                "details_for_account",
                down(),
                None,
                Forward,
                reading.clone(),
            ),
            ("details_for_account", down(), None, Backward, reading),
            (
                "details_for_account",
                down(),
                Some(7),
                Forward,
                repos.clone(),
            ),
            ("details_for_account", down(), Some(5), Backward, repos),
        ];
        for (method, error, start_key, direction, expected) in cases {
            let mut store = seven_details();
            store.fail_next(method, error);
            let outcome = run(&mut store, input(start_key, direction, SCREEN_ROWS));
            assert_eq!(
                outcome,
                Err(expected),
                "{method} {start_key:?} {direction:?}"
            );
            assert_eq!(
                store.calls.last(),
                Some(&method),
                "no read follows {method}"
            );
        }
    }

    #[test]
    fn a_store_that_cannot_be_opened_is_the_psb_scheduling_text() {
        // V-COPAUS0C-010, P-COPAUS0C-020, P-COPAUS0C-021
        assert_eq!(
            schedule_psb_failure(),
            ims(messages::SYSTEM_ERROR_WHILE_SCHEDULING_PSB)
        );
    }

    #[test]
    fn the_rule_future_is_send_over_a_send_store() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = seven_details();
        let future = summary(&mut store, input(None, Direction::Forward, SCREEN_ROWS));
        assert_send(&future);
        assert!(block_on(future).is_ok());
    }
}
