//! COPAUS0C, the pending-authorization summary: the service of endpoint 21,
//! `GET /api/v1/accounts/{acct_id}/authorizations` (transaction `CPVS`, map `COPAU0A`;
//! AAP 0.3.4.1), called only by `api::handlers::copaus0c`.
//!
//! [`summary`] performs the reads of `GATHER-DETAILS` and `PROCESS-PAGE-FORWARD` through the
//! repository traits and hands every outcome to `domain::online::copaus0c`, which decides it:
//!
//! 1. `PROCESS-ENTER-KEY`: the raw `acct_id` path segment through `copaus0c::edit_account`,
//!    before anything is read (COPAUS0C-005, COPAUS0C-006, COPAUS0C-008).
//! 2. `GATHER-ACCOUNT-DETAILS`: `GETCARDXREF-BYACCT`, `GETACCTDATA-BYACCT` and
//!    `GETCUSTDATA-BYCUST` as one `AccountRepository::find_with_customer` chain, in the
//!    program's order: the cross-reference by account through the `card_xref` `acct_id` index,
//!    then the account, then the customer by the cross-reference's `XREF-CUST-ID`
//!    (COPAUS0C-013 to COPAUS0C-019). The chain keys the account read by the edited account
//!    rather than by `XREF-ACCT-ID`; the cross-reference was found by that account, so the two
//!    keys are equal.
//! 3. `GET-AUTH-SUMMARY`, the DL/I `GU` of the root segment: the summary by the account as the
//!    signed `PA-ACCT-ID PIC S9(11) COMP-3` key, through the domain's checked `From`
//!    conversion `copaus0c::summary_key` (COPAUS0C-027 to COPAUS0C-030).
//! 4. Only with a summary, `PROCESS-PAGE-FORWARD`: one forward, index-served page query from
//!    the key the request names, with the domain's lookahead (COPAUS0C-031, COPAUS0C-036,
//!    COPAUS0C-041, COPAUS0C-047). Without a summary no detail is read and the list is empty
//!    (COPAUS0C-029).
//!
//! The COBOL performs every read even after one fails; endpoint 21 stops at the first failed
//! read and returns that read's message alone, which the domain functions return as an
//! `AuthSummaryFailure`. On F7 and F8 the COBOL re-reads only the summary, while endpoint 21
//! runs the whole chain on every request, because its response always carries the block; the
//! screen does not display the block an F7 or F8 returns (COPAUS0C-048). The top-of-page and
//! bottom-of-page messages are the screen's, decided from its paging state, and never produced
//! here.
//!
//! # Store errors
//!
//! Every read this service performs has an audited message for its failure: the cross-reference,
//! account and customer system-error texts (COPAUS0C-015, COPAUS0C-017, COPAUS0C-019), the
//! summary's (COPAUS0C-030) and the detail reads' (COPAUS0C-037, COPAUS0C-038). Each store error
//! therefore becomes the domain's failed lookup or `copaus0c::detail_failure` and reaches the
//! client as `ServiceError::Rule`; the codes those texts embed are `HALTED(H4)` (COPAUS0C-056).
//! `ServiceError::Store` is never produced, and `ServiceError::Internal` only for a broken
//! internal contract (see [`summary`]).
//!
//! # Redaction
//!
//! The outcome holds no card number, full or redacted: the cross-reference's and the details'
//! card numbers stay inside the records, which never leave this module (COPAUS0C-055). Each
//! failed read is logged once by [`traced`], with its paragraph, its statement and the store
//! error, before it is mapped; the error's `Display` names an SQLSTATE and a constraint or column
//! at most. No event names a key, a record or a database text, and none of them reaches the
//! response (Cardholder Data Redaction).

use domain::copybook::PicU;
use domain::online::copaus0c::{
    self, AuthKey, AuthPage, AuthSummaryFailure, Lookup, PageRequest, SummaryBlock, SummaryState,
};
use persistence::{
    AccountCustomerChain, AccountRepository, ChainLink, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PersistenceError,
};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::copaus0c::AuthSummaryQuery;
use crate::dto::resolve_page_size;
use crate::error::ServiceError;

/// The program the tracing event of a failed read names.
const PROGRAM: &str = "COPAUS0C";

/// Endpoint 21: the edited account, its customer and summary block and one page of its pending
/// authorizations, which `AuthSummaryResponse::from` turns into the response (COPAUS0C-008,
/// COPAUS0C-020 to COPAUS0C-036, COPAUS0C-041, COPAUS0C-047, COPAUS0C-052).
///
/// `acct_id` is the path segment as received. `query` is ENTER or a first entry without a
/// `page_key`, which shows the first page whatever the direction; F8 with the displayed page's
/// last key and `direction` forward, or absent; F7 with the first key the screen saved for the
/// previous page and `direction` backward, as COPAUS0C's COMMAREA kept it
/// (`CDEMO-CPVS-PAUKEY-PREV-PG`). The page holds `page_size` rows, or the 5 rows of the map when
/// it is absent (Bounded Pagination). The signed-on user is not consulted: COPAUS0C performs no
/// check of its own on the user (AAP 0.5.1).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the domain's failure, in the program's order: the account edit
///   (400, COPAUS0C-005, COPAUS0C-006); the cross-reference read (404 `NOTFND` on field
///   `acct_id`, or 500, COPAUS0C-014, COPAUS0C-015); the account read (404 or 500, COPAUS0C-016,
///   COPAUS0C-017); the customer read (404 or 500, COPAUS0C-018, COPAUS0C-019); the summary read
///   (500, COPAUS0C-030); the detail query (500, COPAUS0C-037 for the first page, COPAUS0C-038
///   for F7 and F8).
/// - [`ServiceError::Internal`] for a `page_key` that is not a rendered authorization key, which
///   `ValidQuery` has already refused with 400 before the handler runs, so reaching it means the
///   query bypassed its validation; and for a query limit that leaves `i64`, which no page size
///   validation admits can produce. Either is a broken internal contract with no COBOL text,
///   answered 500 with no value.
pub(crate) async fn summary(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
    query: AuthSummaryQuery,
) -> Result<(PicU<11>, SummaryBlock, AuthPage), ServiceError<AuthSummaryFailure>> {
    let AuthSummaryQuery {
        page_key,
        direction,
        page_size,
    } = query;
    let acct = copaus0c::edit_account(acct_id).map_err(ServiceError::Rule)?;
    // The key is untrusted input; `AuthKey::parse` is total, and its refusal carries no text.
    let page_key = page_key
        .as_deref()
        .map(AuthKey::parse)
        .transpose()
        .map_err(|_| ServiceError::Internal)?;
    let request = PageRequest::new(direction.map(copaus0c::Direction::from), page_key);

    let mut store = state.store.clone();
    let chain = AccountRepository::find_with_customer(&mut store, acct).await;
    report_failed_reads(&chain);
    let keys = copaus0c::xref_keys(acct, chain_lookup(&chain.xref)).map_err(ServiceError::Rule)?;
    let summary_key = copaus0c::summary_key(acct);
    let summary = PendingAuthSummaryRepository::find_by_id(&mut store, summary_key).await;
    let block = copaus0c::summary_block(
        keys,
        chain_lookup(&chain.account),
        chain_lookup(&chain.customer),
        read_lookup(&summary),
    )
    .map_err(ServiceError::Rule)?;

    let page = match block.summary {
        SummaryState::Found => {
            let rows = page_rows(page_size);
            // TRUNCATION: checked, never lossy: `page_rows` gives at most 100 rows, so the limit,
            // the rows plus the anchor and the lookahead, is at most 102; a limit `i64` cannot
            // hold is refused, never wrapped.
            let limit =
                i64::try_from(request.fetch_limit(rows)).map_err(|_| ServiceError::Internal)?;
            let start = request.start_key();
            PendingAuthDetailRepository::page_after(
                &mut store,
                summary_key,
                start.date_9c(),
                start.time_9c(),
                limit,
            )
            .await
            .map(|details| copaus0c::page(request, &details, rows))
            .map_err(|error| detail_read_failed(request, &error))?
        }
        SummaryState::NotFound => AuthPage::without_details(),
    };
    Ok((acct, block, page))
}

/// Logs each read of `chain` that failed through [`traced`], with its paragraph and dataset;
/// a read that found its record or none, or was never issued, logs nothing (COPAUS0C-015,
/// COPAUS0C-017, COPAUS0C-019).
fn report_failed_reads(chain: &AccountCustomerChain) {
    report_failed(&chain.xref, "GETCARDXREF-BYACCT", "READ CXACAIX");
    report_failed(&chain.account, "GETACCTDATA-BYACCT", "READ ACCTDAT");
    report_failed(&chain.customer, "GETCUSTDATA-BYCUST", "READ CUSTDAT");
}

/// Logs `link` through [`traced`] when its read failed.
fn report_failed<T>(link: &ChainLink<T>, paragraph: &'static str, statement: &'static str) {
    if let ChainLink::Failed(error) = link {
        traced(paragraph, statement, error);
    }
}

/// The answer to a failed detail page query, `copaus0c::detail_failure` for `request`, once the
/// error is logged (COPAUS0C-037, COPAUS0C-038).
fn detail_read_failed(
    request: PageRequest,
    error: &PersistenceError,
) -> ServiceError<AuthSummaryFailure> {
    traced("PROCESS-PAGE-FORWARD", "GNP PAUTDTL1", error);
    ServiceError::Rule(copaus0c::detail_failure(request))
}

/// The tracing event of a failed read, in place of the response and reason codes the COBOL puts
/// in its system-error text (`HALTED(H4)`, COPAUS0C-056).
///
/// `paragraph` and `statement` are constants of this module; the error's `Display` holds only
/// an SQLSTATE and a constraint or column name, never a key or a stored value (Cardholder Data
/// Redaction).
fn traced(paragraph: &'static str, statement: &'static str, error: &PersistenceError) {
    tracing::warn!(
        program = PROGRAM,
        paragraph,
        statement,
        error = %error,
        "read failed; answering the program's system error"
    );
}

/// One link of the `find_with_customer` chain as the read outcome the domain judges: a record
/// is found, `NOTFND` is not found, and any other response is a failed read (COPAUS0C-013 to
/// COPAUS0C-019).
///
/// A [`ChainLink::NotReached`] link is a read that was never issued, so it yielded no record and
/// counts as a failed read, the system error, never as a not-found that would claim the record
/// absent. The domain judges the account and customer links only after a found cross-reference,
/// and the chain issues both reads whenever the cross-reference is found, so no response is built
/// from a `NotReached` link.
fn chain_lookup<T>(link: &ChainLink<T>) -> Lookup<&T> {
    match link {
        ChainLink::Found(record) => Lookup::Found(record),
        ChainLink::NotFound => Lookup::NotFound,
        ChainLink::Failed(_) | ChainLink::NotReached => Lookup::Failed,
    }
}

/// A keyed read as the read outcome the domain judges: the DL/I `GU` of the summary, whose IMS
/// status `'  '` or `'FW'` is a found row, `'GE'` no row, and any other status a failed read
/// (COPAUS0C-027, COPAUS0C-029, COPAUS0C-030), whose error is logged first through [`traced`].
fn read_lookup<T>(read: &Result<Option<T>, PersistenceError>) -> Lookup<&T> {
    match read {
        Ok(Some(record)) => Lookup::Found(record),
        Ok(None) => Lookup::NotFound,
        Err(error) => {
            traced("GET-AUTH-SUMMARY", "GU PAUTSUM0", error);
            Lookup::Failed
        }
    }
}

/// The rows of the page: `requested`, or the map's 5 rows when it is absent (COPAUS0C-031;
/// Bounded Pagination).
fn page_rows(requested: Option<i64>) -> usize {
    // TRUNCATION: checked both ways, never lossy: the screen size 5 fits `u32`, validation admits
    // only 1 to 100, which `usize` holds, and a conversion that fails gives the screen size.
    u32::try_from(copaus0c::DEFAULT_PAGE_SIZE)
        .ok()
        .map(|default| resolve_page_size(requested, default))
        .and_then(|rows| usize::try_from(rows).ok())
        .unwrap_or(copaus0c::DEFAULT_PAGE_SIZE)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::CardNumber;
    use domain::copybook::{PicU, PicX};
    use domain::online::copaus0c::{self, DEFAULT_PAGE_SIZE, Lookup, PageRequest};
    use domain::records::card_xref::CardXrefRecord;
    use persistence::{AccountCustomerChain, ChainLink, PersistenceError};
    use test_support::trace_capture::{self, TraceCapture};

    use super::{chain_lookup, detail_read_failed, page_rows, read_lookup, report_failed_reads};
    use crate::error::ServiceError;

    /// A keyed read of a stand-in record.
    type Read = Result<Option<u8>, PersistenceError>;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A store error carrying an SQLSTATE and a constraint name.
    fn store_error() -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some("57014".to_owned()),
            constraint: Some("pending_auth_summaries_pkey".to_owned()),
        }
    }

    /// Asserts that `traces` holds exactly the events `logged` names, one line each, in order:
    /// a `WARN` of COPAUS0C with the paragraph, the statement and the error's text.
    fn assert_logged(traces: &TraceCapture, logged: &[(&str, &str, &str)]) {
        let lines = traces.lines();
        assert_eq!(lines.len(), logged.len(), "{}", traces.text());
        for (line, &(paragraph, statement, cause)) in lines.iter().zip(logged) {
            let paragraph = format!(r#""paragraph":"{paragraph}""#);
            let statement = format!(r#""statement":"{statement}""#);
            for expected in [
                r#""level":"WARN""#,
                r#""program":"COPAUS0C""#,
                paragraph.as_str(),
                statement.as_str(),
                cause,
            ] {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
        }
    }

    /// COPAUS0C-015, COPAUS0C-017, COPAUS0C-019: each failed read of the chain is logged once
    /// with its paragraph and the store error; a found, missing or unissued read logs nothing,
    /// and no event holds a key or a stored value.
    #[test]
    fn each_failed_chain_read_is_logged_once_without_any_value() -> TestResult {
        let xref = CardXrefRecord {
            card_num: CardNumber::parse("4111111111111111")?,
            cust_id: PicU::new(7)?,
            acct_id: PicU::new(42)?,
            filler: PicX::spaces(),
        };
        let traces = trace_capture::capture();
        let cases = [
            (
                AccountCustomerChain {
                    xref: ChainLink::Failed(PersistenceError::Unavailable),
                    account: ChainLink::NotReached,
                    customer: ChainLink::NotReached,
                },
                &[("GETCARDXREF-BYACCT", "READ CXACAIX", "database unavailable")][..],
            ),
            (
                AccountCustomerChain {
                    xref: ChainLink::Found(xref.clone()),
                    account: ChainLink::Failed(store_error()),
                    customer: ChainLink::Failed(PersistenceError::Unavailable),
                },
                &[
                    ("GETACCTDATA-BYACCT", "READ ACCTDAT", "SQLSTATE 57014"),
                    ("GETCUSTDATA-BYCUST", "READ CUSTDAT", "database unavailable"),
                ][..],
            ),
            (
                AccountCustomerChain {
                    xref: ChainLink::Found(xref),
                    account: ChainLink::NotFound,
                    customer: ChainLink::NotFound,
                },
                &[][..],
            ),
        ];
        for (chain, logged) in cases {
            traces.clear();
            report_failed_reads(&chain);
            assert_logged(&traces, logged);
        }
        for value in ["4111111111111111", "00000000042", "000000007"] {
            assert!(!traces.contains(value), "the trace holds {value}");
        }
        Ok(())
    }

    /// COPAUS0C-030, COPAUS0C-037: a failed summary `GU` and a failed detail page are logged
    /// once each and keep their Rule answers; a found or missing summary logs nothing.
    #[test]
    fn a_failed_summary_or_detail_read_is_logged_once_and_keeps_its_answer() {
        let traces = trace_capture::capture();
        let failed: Read = Err(store_error());
        assert_eq!(read_lookup(&failed), Lookup::Failed);
        assert_logged(
            &traces,
            &[("GET-AUTH-SUMMARY", "GU PAUTSUM0", "SQLSTATE 57014")],
        );

        traces.clear();
        let answer = detail_read_failed(PageRequest::First, &PersistenceError::Unavailable);
        assert_eq!(
            answer,
            ServiceError::Rule(copaus0c::detail_failure(PageRequest::First))
        );
        assert_logged(
            &traces,
            &[(
                "PROCESS-PAGE-FORWARD",
                "GNP PAUTDTL1",
                "database unavailable",
            )],
        );

        traces.clear();
        let found: Read = Ok(Some(7));
        let missing: Read = Ok(None);
        assert_eq!(read_lookup(&found), Lookup::Found(&7));
        assert_eq!(read_lookup(&missing), Lookup::NotFound);
        assert_logged(&traces, &[]);
    }

    #[test]
    fn every_chain_link_maps_to_the_lookup_the_domain_judges() {
        let cases: [(ChainLink<u8>, Lookup<&u8>); 4] = [
            (ChainLink::Found(7), Lookup::Found(&7)),
            (ChainLink::NotFound, Lookup::NotFound),
            (
                ChainLink::Failed(PersistenceError::Unavailable),
                Lookup::Failed,
            ),
            (ChainLink::NotReached, Lookup::Failed),
        ];
        for (link, expected) in &cases {
            assert_eq!(chain_lookup(link), *expected);
        }
    }

    #[test]
    fn every_keyed_read_maps_to_the_lookup_the_domain_judges() {
        let cases: [(Read, Lookup<&u8>); 3] = [
            (Ok(Some(7)), Lookup::Found(&7)),
            (Ok(None), Lookup::NotFound),
            (Err(PersistenceError::Unavailable), Lookup::Failed),
        ];
        for (read, expected) in &cases {
            assert_eq!(read_lookup(read), *expected);
        }
    }

    #[test]
    fn the_page_holds_the_requested_rows_or_the_maps_five() {
        for (requested, expected) in [
            (None, DEFAULT_PAGE_SIZE),
            (Some(1), 1),
            (Some(5), 5),
            (Some(100), 100),
            (Some(-1), DEFAULT_PAGE_SIZE),
        ] {
            assert_eq!(page_rows(requested), expected, "requested {requested:?}");
        }
    }
}
