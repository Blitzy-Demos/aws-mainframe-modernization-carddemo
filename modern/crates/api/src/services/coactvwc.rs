//! COACTVWC, account view: the service of endpoint 4, `GET /api/v1/accounts/{acct_id}/view`
//! (transaction `CAVW`, AAP 0.3.4.1), called only by `api::handlers::coactvwc`.
//!
//! [`view`] runs the program's ENTER path in its order:
//!
//! 1. `2200-EDIT-MAP-INPUTS` and `2210-EDIT-ACCOUNT`: the raw `acct_id` path text goes through
//!    [`coactvwc::edit_account`], which is total and owns the edit's messages (COACTVWC-029 to
//!    COACTVWC-032). A failed edit reads nothing.
//! 2. `9000-READ-ACCT`: [`AccountRepository::find_with_customer`] issues the cross-reference
//!    read by account through `CXACAIX`, then the account read by the same key, then the
//!    customer read by the cross-reference's customer ID (`COACTVWC.cbl:687-870`;
//!    COACTVWC-033, COACTVWC-034).
//! 3. [`coactvwc::view`] decides the view or the failure from the three outcomes, with the
//!    program's message precedence (COACTVWC-035 to COACTVWC-047).
//!
//! # Store outcomes
//!
//! The chain is infallible: each read's failure travels in its own link, and [`lookup`] turns
//! every link into the domain's [`Lookup`]. A failed read becomes [`Lookup::Error`], the
//! `WHEN OTHER` branch whose `File Error:` text the domain renders, with its CICS codes
//! `HALTED(H4)` (COACTVWC-037, COACTVWC-038, COACTVWC-042, COACTVWC-043, COACTVWC-046,
//! COACTVWC-047). The [`PersistenceError`] it carries is logged
//! once by [`traced`], with the read's paragraph and statement, in place of the codes the COBOL
//! puts in that text, and never reaches a message, so no outcome of this endpoint is a
//! [`ServiceError::Store`].
//!
//! # Identity, writes and cardholder data
//!
//! - No paragraph reads the signed-on user ID or type: the extractor requires a valid token, and
//!   the response is the same for every user type (COACTVWC-002).
//! - The program issues only `READ` commands, so the reads run on the pool in autocommit and
//!   nothing is written (COACTVWC-005).
//! - The SSN stays a `domain::sensitive::Ssn` inside the [`AccountView`], and the card number
//!   the cross-reference read returns is dropped with the chain (COACTVWC-017, COACTVWC-034,
//!   E32). Nothing here formats, logs or traces a record or a key; the event of a failed read
//!   carries only the store error, whose `Display` names an SQLSTATE and a constraint or column
//!   at most (Cardholder Data Redaction).

use domain::copybook::PicU;
use domain::online::coactvwc::{self, AccountView, Lookup, ViewFailure};
use persistence::{AccountCustomerChain, AccountRepository, ChainLink, PersistenceError};
use tracing::callsite::DefaultCallsite;
use tracing::field::{Value, display};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::error::ServiceError;

/// The program the tracing event of a failed read names.
const PROGRAM: &str = "COACTVWC";

/// ENTER on the account view: the edit of the typed account number, the reads of
/// `9000-READ-ACCT` and the view `1200-SETUP-SCREEN-VARS` shows (COACTVWC-005, COACTVWC-008,
/// COACTVWC-029 to COACTVWC-047).
///
/// `acct_id` is the path segment exactly as the client sent it; `_user` is the signed-on user,
/// whom the program never consults (COACTVWC-002).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the program's [`ViewFailure`]: the account-number edit failed
/// (400, COACTVWC-030, COACTVWC-031), a read found no record (404, COACTVWC-035, COACTVWC-040,
/// COACTVWC-044), or a read failed (500, COACTVWC-037, COACTVWC-039, COACTVWC-042,
/// COACTVWC-046).
pub(crate) async fn view(
    state: &AppState,
    _user: &SignedOnUser,
    acct_id: &str,
) -> Result<AccountView, ServiceError<ViewFailure>> {
    let acct = coactvwc::edit_account(acct_id).map_err(ServiceError::Rule)?;
    // The repository methods take `&mut self`; a clone copies only the pool handle.
    let mut store = state.store.clone();
    let chain = AccountRepository::find_with_customer(&mut store, acct).await;
    decide(acct, &chain).map_err(ServiceError::Rule)
}

/// The domain's decision over the three reads of `chain` for account `acct_id`, each link
/// mapped by [`lookup`] (COACTVWC-033), after each failed read is logged once.
fn decide(acct_id: PicU<11>, chain: &AccountCustomerChain) -> Result<AccountView, ViewFailure> {
    report_failed(&chain.xref, "9200-GETCARDXREF-BYACCT", "READ CXACAIX");
    report_failed(&chain.account, "9300-GETACCTDATA-BYACCT", "READ ACCTDAT");
    report_failed(&chain.customer, "9400-GETCUSTDATA-BYCUST", "READ CUSTDAT");
    coactvwc::view(
        acct_id,
        lookup(&chain.xref),
        lookup(&chain.account),
        lookup(&chain.customer),
    )
}

/// Logs `link` through [`traced`] when its read failed; any other link logs nothing.
fn report_failed<T>(link: &ChainLink<T>, paragraph: &'static str, statement: &'static str) {
    if let ChainLink::Failed(error) = link {
        traced(paragraph, statement, error);
    }
}

/// The tracing event of a failed read, in place of the `RESP` and `RESP2` codes the `File
/// Error:` text holds in the COBOL (`HALTED(H4)`; COACTVWC-037, COACTVWC-042, COACTVWC-046).
///
/// `paragraph` and `statement` are constants of this module; the error's `Display` holds only
/// an SQLSTATE and a constraint or column name, never a key or a stored value (Cardholder Data
/// Redaction).
fn traced(paragraph: &'static str, statement: &'static str, error: &PersistenceError) {
    // Written out: `tracing`'s `error!` expands to an `allow(unused_imports)` lint suppression.
    static EVENT: DefaultCallsite = DefaultCallsite::new(&EVENT_METADATA);
    static EVENT_METADATA: Metadata<'static> = tracing::metadata! {
        name: concat!("event ", file!(), ":", line!()),
        target: module_path!(),
        level: Level::ERROR,
        fields: &["message", "program", "paragraph", "statement", "error"],
        callsite: &EVENT,
        kind: Kind::EVENT,
    };
    if runtime::telemetry::event_enabled(&EVENT) {
        Event::dispatch(
            &EVENT_METADATA,
            &EVENT_METADATA.fields().value_set_all(&[
                Some(&"file read failed; answering the program's file error" as &dyn Value),
                Some(&PROGRAM as &dyn Value),
                Some(&paragraph as &dyn Value),
                Some(&statement as &dyn Value),
                Some(&display(error) as &dyn Value),
            ]),
        );
    }
}

/// One read of the chain as the lookup [`coactvwc::view`] takes: CICS `NORMAL` is
/// [`Lookup::Found`], `NOTFND` is [`Lookup::NotFound`] (COACTVWC-035, COACTVWC-040,
/// COACTVWC-044), and any other response is [`Lookup::Error`], whose error [`decide`] has
/// logged (COACTVWC-037, COACTVWC-039, COACTVWC-042, COACTVWC-046).
fn lookup<T>(link: &ChainLink<T>) -> Lookup<&T> {
    match link {
        ChainLink::Found(record) => Lookup::Found(record),
        // A read the chain never issued, because the cross-reference read did not find exactly
        // one record, is never examined by the domain, which documents `NotFound` for it
        // (COACTVWC-033).
        ChainLink::NotFound | ChainLink::NotReached => Lookup::NotFound,
        ChainLink::Failed(_) => Lookup::Error,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::CardNumber;
    use domain::copybook::{FixedRecord, PicU, PicX};
    use domain::online::coactvwc::{FailureClass, Lookup, ViewField};
    use domain::records::account::AccountRecord;
    use domain::records::card_xref::CardXrefRecord;
    use domain::records::customer::CustomerRecord;
    use persistence::{AccountCustomerChain, ChainLink, PersistenceError};
    use test_support::trace_capture;

    use super::{decide, lookup};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// An SQLSTATE no COACTVWC message may quote.
    const SQLSTATE: &str = "XX000";

    /// A store error carrying an SQLSTATE and a constraint name.
    fn store_error() -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some(SQLSTATE.to_owned()),
            constraint: Some("accounts_pkey".to_owned()),
        }
    }

    /// The three records of account 42 and customer 7: zero amounts, blank dates and names.
    fn records() -> Result<(CardXrefRecord, AccountRecord, CustomerRecord), Box<dyn StdError>> {
        let zero = "00000000000{";
        let account_line = [
            "00000000042",
            "Y",
            zero,
            zero,
            zero,
            &" ".repeat(30),
            zero,
            zero,
            &" ".repeat(198),
        ]
        .concat();
        let customer_line = [
            "000000007",
            &" ".repeat(270),
            "123456789",
            &" ".repeat(40),
            "Y",
            "075",
            &" ".repeat(168),
        ]
        .concat();
        let account = AccountRecord::decode(account_line.as_bytes())?;
        let customer = CustomerRecord::decode(customer_line.as_bytes())?;
        let xref = CardXrefRecord {
            card_num: CardNumber::parse("4111111111111111")?,
            cust_id: customer.id,
            acct_id: account.id,
            filler: PicX::spaces(),
        };
        Ok((xref, account, customer))
    }

    #[test]
    fn every_link_maps_to_the_lookup_the_domain_takes() {
        let cases = [
            (ChainLink::Found(7_u8), Lookup::Found(&7_u8)),
            (ChainLink::NotFound, Lookup::NotFound),
            (ChainLink::NotReached, Lookup::NotFound),
            (ChainLink::Failed(store_error()), Lookup::Error),
            (
                ChainLink::Failed(PersistenceError::Unavailable),
                Lookup::Error,
            ),
        ];
        for (link, expected) in &cases {
            assert_eq!(lookup(link), *expected, "{link:?}");
        }
    }

    /// COACTVWC-008: all three reads found their record, so the view carries both blocks.
    #[test]
    fn a_full_chain_is_the_view_of_the_account() -> TestResult {
        let (xref, account, customer) = records()?;
        let key = PicU::<11>::new(42)?;
        let chain = AccountCustomerChain {
            xref: ChainLink::Found(xref),
            account: ChainLink::Found(account),
            customer: ChainLink::Found(customer),
        };
        let view = decide(key, &chain).map_err(|failure| failure.message().to_owned())?;
        assert_eq!(view.acct_id, key);
        assert_eq!(view.customer.customer_id, PicU::<9>::new(7)?);
        Ok(())
    }

    /// COACTVWC-033, COACTVWC-035 to COACTVWC-046: each link reaches its own read's rule, a
    /// read the chain never issued is not examined, and a store error reaches no message.
    #[test]
    fn every_chain_outcome_reaches_its_reads_rule() -> TestResult {
        let (xref, account, customer) = records()?;
        let key = PicU::<11>::new(42)?;
        let cases = [
            // The chain ended at the cross-reference read (COACTVWC-035, COACTVWC-037).
            (
                ChainLink::NotFound,
                ChainLink::NotReached,
                ChainLink::NotReached,
                FailureClass::NotFound,
                Some(ViewField::AcctId),
                "Cross ref file",
            ),
            (
                ChainLink::Failed(store_error()),
                ChainLink::NotReached,
                ChainLink::NotReached,
                FailureClass::Store,
                Some(ViewField::AcctId),
                "CXACAIX",
            ),
            // The account read did not succeed (COACTVWC-040, COACTVWC-042).
            (
                ChainLink::Found(xref.clone()),
                ChainLink::NotFound,
                ChainLink::Found(customer.clone()),
                FailureClass::NotFound,
                Some(ViewField::AcctId),
                "Acct Master file",
            ),
            (
                ChainLink::Found(xref.clone()),
                ChainLink::Failed(store_error()),
                ChainLink::NotFound,
                FailureClass::Store,
                Some(ViewField::AcctId),
                "ACCTDAT",
            ),
            // The customer read did not succeed (COACTVWC-044, COACTVWC-046).
            (
                ChainLink::Found(xref.clone()),
                ChainLink::Found(account.clone()),
                ChainLink::NotFound,
                FailureClass::NotFound,
                None,
                "customer master",
            ),
            (
                ChainLink::Found(xref),
                ChainLink::Found(account),
                ChainLink::Failed(store_error()),
                FailureClass::Store,
                None,
                "CUSTDAT",
            ),
        ];
        for (xref, account, customer, class, field, names) in cases {
            let chain = AccountCustomerChain {
                xref,
                account,
                customer,
            };
            let failure = decide(key, &chain)
                .err()
                .ok_or("an incomplete chain gave a view")?;
            let message = failure.message();
            assert_eq!(
                (failure.class(), failure.field()),
                (class, field),
                "{message}"
            );
            assert!(message.contains(names), "{message}");
            assert!(!message.contains(SQLSTATE) && !message.contains("accounts_pkey"));
        }
        Ok(())
    }

    /// COACTVWC-037, COACTVWC-042, COACTVWC-046: each failed read is logged once, as an `ERROR`
    /// event of COACTVWC with its paragraph, its statement and the store error, and the view
    /// answers with the program's file error; a read that found its record or none, or that the
    /// chain never issued, logs nothing, and no event holds a key or a stored value.
    #[test]
    fn each_failed_read_is_logged_once_without_any_value() -> TestResult {
        let (xref, account, customer) = records()?;
        let key = PicU::<11>::new(42)?;
        let traces = trace_capture::capture();
        let cases = [
            (
                ChainLink::Failed(store_error()),
                ChainLink::NotReached,
                ChainLink::NotReached,
                FailureClass::Store,
                &[("9200-GETCARDXREF-BYACCT", "READ CXACAIX", "SQLSTATE XX000")][..],
            ),
            (
                ChainLink::Found(xref.clone()),
                ChainLink::Failed(store_error()),
                ChainLink::Failed(PersistenceError::Unavailable),
                FailureClass::Store,
                &[
                    ("9300-GETACCTDATA-BYACCT", "READ ACCTDAT", "SQLSTATE XX000"),
                    (
                        "9400-GETCUSTDATA-BYCUST",
                        "READ CUSTDAT",
                        "database unavailable",
                    ),
                ][..],
            ),
            (
                ChainLink::Found(xref.clone()),
                ChainLink::Failed(store_error()),
                ChainLink::Found(customer),
                FailureClass::Store,
                &[("9300-GETACCTDATA-BYACCT", "READ ACCTDAT", "SQLSTATE XX000")][..],
            ),
            (
                ChainLink::Found(xref),
                ChainLink::Found(account),
                ChainLink::NotFound,
                FailureClass::NotFound,
                &[][..],
            ),
        ];
        for (xref, account, customer, class, logged) in cases {
            traces.clear();
            let chain = AccountCustomerChain {
                xref,
                account,
                customer,
            };
            let failure = decide(key, &chain)
                .err()
                .ok_or("an incomplete chain gave a view")?;
            assert_eq!(failure.class(), class, "{}", failure.message());
            let lines = traces.lines();
            assert_eq!(lines.len(), logged.len(), "{}", traces.text());
            for (line, &(paragraph, statement, cause)) in lines.iter().zip(logged) {
                let paragraph = format!(r#""paragraph":"{paragraph}""#);
                let statement = format!(r#""statement":"{statement}""#);
                for expected in [
                    r#""level":"ERROR""#,
                    r#""program":"COACTVWC""#,
                    paragraph.as_str(),
                    statement.as_str(),
                    cause,
                ] {
                    assert!(line.contains(expected), "{expected} missing from {line}");
                }
            }
        }
        for value in ["4111111111111111", "00000000042", "123456789", "000000007"] {
            assert!(!traces.contains(value), "the trace holds {value}");
        }
        Ok(())
    }
}
