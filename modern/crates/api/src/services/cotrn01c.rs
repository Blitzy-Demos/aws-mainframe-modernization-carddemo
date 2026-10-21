//! COTRN01C, the transaction view: the service of endpoint 12,
//! `GET /api/v1/transactions/{tran_id}` (transaction `CT01`, map `COTRN1A`; AAP 0.3.4.1).
//!
//! [`view`] ports `PROCESS-ENTER-KEY` and its `READ-TRANSACT-FILE` (`COTRN01C.cbl:143-192`,
//! `COTRN01C.cbl:267-295`), the program's only read. `api::handlers::cotrn01c` is its only
//! caller; it extracts the signed-on user and the raw path parameter, calls [`view`] and maps
//! the answer through `TranViewResponse::from` or `ScreenError::from` (Thin Request Handlers).
//!
//! # Flow
//!
//! 1. The raw `tran_id` goes through `cotrn01c::edit_tran_id`, which is total: a Tran ID that
//!    is all spaces or all low-values answers `Tran ID can NOT be empty...` (400) with no read
//!    (COTRN01C-009); any other text becomes the 16-byte key exactly as entered (COTRN01C-010).
//! 2. One keyed read of `transactions` through `TransactionRepository::find_by_id`
//!    (COTRN01C-011).
//! 3. What the read found becomes a `TranLookup` ([`lookup`]): a row is `Found` (COTRN01C-015),
//!    no row is `NotFound`, the `DFHRESP(NOTFND)` answered with `Transaction ID NOT found...`
//!    (404; COTRN01C-012), and any store error is `Error`, the `WHEN OTHER` answered with
//!    `Unable to lookup Transaction...` (500; COTRN01C-013).
//! 4. `cotrn01c::view` turns the lookup into the `TranView` of the map's detail fields
//!    (COTRN01C-016 to COTRN01C-028) or into its failure, which [`view`] returns as
//!    `ServiceError::Rule`.
//!
//! # Store errors
//!
//! COTRN01C answers every failed read with a text of its own, so a store error on the read is a
//! domain lookup value and never a `ServiceError::Store`: the read's result is matched, never
//! propagated with `?`. The COBOL `DISPLAY`s its CICS `RESP` and `RESP2` to the region log,
//! codes that have no counterpart here (COTRN01C-013); in their place [`lookup`] logs the store
//! error once, as a tracing event naming `READ-TRANSACT-FILE`, before it becomes the lookup
//! value.
//!
//! # No lock, no write
//!
//! The COBOL reads `TRANSACT` with `UPDATE` but never rewrites, deletes or unlocks the record,
//! so its lock lasts only until the task ends. No write follows here either, so the read runs on
//! the pool in autocommit mode, takes no row lock and every table is unchanged after any call,
//! whatever its outcome (COTRN01C-011).
//!
//! # Identity
//!
//! COTRN01C checks no user type: a `U` and an `A` token get the same view (COTRN01C-015). The
//! signed-on user is taken so that the service runs only for a verified token (AAP 0.5.1) and
//! is not read otherwise.
//!
//! # Redaction
//!
//! The card number stays a `CardNumber` inside the returned view, so the response carries only
//! its redacted form (COTRN01C-017, E32, Cardholder Data Redaction). Nothing here logs, traces or
//! formats the key or the record; the event of a failed read carries only the store error, whose
//! `Display` names an SQLSTATE and a constraint or column at most.

use domain::online::cotrn01c::{self, TranLookup, TranView, TranViewFailure};
use domain::records::TransactionRecord;
use persistence::{PersistenceError, TransactionRepository};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::error::ServiceError;

/// The transaction `tran_id` names, as the `COTRN1A` map shows it after ENTER: endpoint 12
/// (COTRN01C-009 to COTRN01C-028).
///
/// `tran_id` is the raw path parameter, the Tran ID input `TRNIDIN` as typed. The same view
/// answers every signed-on user, whatever the user type (COTRN01C-015). One read, no lock and
/// no write (COTRN01C-011).
///
/// # Errors
///
/// [`ServiceError::Rule`] carrying:
///
/// - [`TranViewFailure::Blank`] (400) when the Tran ID is blank; nothing is read
///   (COTRN01C-009).
/// - [`TranViewFailure::NotFound`] (404) when no transaction has the key, a text no 16-byte key
///   can equal included (COTRN01C-010, COTRN01C-012).
/// - [`TranViewFailure::LookupFailed`] (500) when the read fails (COTRN01C-013).
pub(crate) async fn view(
    state: &AppState,
    _user: &SignedOnUser,
    tran_id: &str,
) -> Result<TranView, ServiceError<TranViewFailure>> {
    match cotrn01c::edit_tran_id(tran_id) {
        Err(failure) => Err(ServiceError::Rule(failure)),
        Ok(id) => {
            // A clone copies the pool handle only; the read runs and commits on its own.
            let mut store = state.store.clone();
            let read = TransactionRepository::find_by_id(&mut store, &id).await;
            cotrn01c::view(lookup(&read)).map_err(ServiceError::Rule)
        }
    }
}

/// The `EXEC CICS READ` response the result of `find_by_id` stands for: `DFHRESP(NORMAL)` with
/// the row, `DFHRESP(NOTFND)`, or any other response (COTRN01C-012, COTRN01C-013, COTRN01C-015).
///
/// The store error does not choose the answer: every failure, whatever its SQLSTATE, gets the
/// program's one `WHEN OTHER` answer (COTRN01C-013). It is logged first, once, as a `WARN` event
/// naming the program and the paragraph, in place of the COBOL's `DISPLAY` of the CICS codes.
fn lookup(read: &Result<Option<TransactionRecord>, PersistenceError>) -> TranLookup<'_> {
    match read {
        Ok(Some(record)) => TranLookup::Found(record),
        Ok(None) => TranLookup::NotFound,
        Err(error) => {
            // The error's `Display` holds only an SQLSTATE and a constraint or column name,
            // never the key or a stored value (Cardholder Data Redaction).
            tracing::warn!(
                program = "COTRN01C",
                paragraph = "READ-TRANSACT-FILE",
                error = %error,
                "TRANSACT read failed; answering Unable to lookup Transaction"
            );
            TranLookup::Error
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use axum::http::StatusCode;
    use domain::copybook::FixedRecord;
    use domain::online::cotrn01c::{self, TranLookup, TranViewFailure};
    use domain::records::TransactionRecord;
    use persistence::{DecodeKind, PersistenceError};
    use test_support::trace_capture;

    use super::lookup;
    use crate::error::{ScreenError, ServiceError, UNABLE_TO_PROCESS};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `TRAN-ID` of `dailytran.txt` line 1.
    const TRAN_ID: &str = "0000000000683580";

    /// `dailytran.txt` line 1 as a `transactions` record: `CVTRA06Y` and `CVTRA05Y` share one
    /// layout, so the line decodes as `TRAN-RECORD`.
    fn first_fixture_record() -> Result<TransactionRecord, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/dailytran.txt");
        let bytes = std::fs::read(path)?;
        let line = bytes
            .split(|&byte| byte == b'\n')
            .next()
            .ok_or("dailytran.txt is empty")?;
        let body = line.strip_suffix(b"\r").unwrap_or(line);
        Ok(TransactionRecord::decode(body)?)
    }

    /// Every store error the read can return, each of which the program answers alike.
    fn store_errors() -> [PersistenceError; 7] {
        [
            PersistenceError::Unavailable,
            PersistenceError::NotFound,
            PersistenceError::Database {
                sqlstate: Some("57014".to_owned()),
                constraint: None,
            },
            PersistenceError::Database {
                sqlstate: None,
                constraint: None,
            },
            PersistenceError::Duplicate {
                constraint: Some("transactions_pkey".to_owned()),
            },
            PersistenceError::Check { constraint: None },
            PersistenceError::Decode {
                column: "tran_id",
                kind: DecodeKind::InvalidByte,
            },
        ]
    }

    // audit: COTRN01C-015
    #[test]
    fn a_found_row_is_the_normal_response_and_shows_that_row() -> TestResult {
        let record = first_fixture_record()?;
        let read = Ok(Some(record.clone()));
        let found = lookup(&read);
        assert_eq!(found, TranLookup::Found(&record));
        let view = cotrn01c::view(found)?;
        assert_eq!(view.tran_id.as_bytes(), TRAN_ID.as_bytes());
        Ok(())
    }

    // audit: COTRN01C-012
    #[test]
    fn no_row_is_notfnd() {
        let read = Ok(None);
        assert_eq!(lookup(&read), TranLookup::NotFound);
        assert_eq!(
            cotrn01c::view(lookup(&read)),
            Err(TranViewFailure::NotFound)
        );
    }

    // audit: COTRN01C-013
    #[test]
    fn every_store_error_is_the_programs_lookup_failure_never_a_store_error() {
        for error in store_errors() {
            let read = Err(error.clone());
            assert_eq!(lookup(&read), TranLookup::Error, "{error:?}");
            let answer = cotrn01c::view(lookup(&read)).map_err(ServiceError::Rule);
            assert_eq!(
                answer,
                Err(ServiceError::Rule(TranViewFailure::LookupFailed)),
                "{error:?}"
            );
        }
        // The program's own 500 text, never the crate's text for an unaudited store error.
        let response = ScreenError::from(ServiceError::Rule(TranViewFailure::LookupFailed));
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.message(), "Unable to lookup Transaction...");
        assert_ne!(response.message(), UNABLE_TO_PROCESS);
        assert_eq!(response.field(), Some("tran_id"));
    }

    // audit: COTRN01C-013
    #[test]
    fn a_failed_read_is_logged_once_with_its_cause_and_a_found_row_is_not() -> TestResult {
        let traces = trace_capture::capture();
        for error in store_errors() {
            traces.clear();
            let cause = error.to_string();
            let read = Err(error);
            assert_eq!(
                cotrn01c::view(lookup(&read)),
                Err(TranViewFailure::LookupFailed),
                "{cause}"
            );
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("{cause}: {} lines", lines.len()).into());
            };
            for expected in [
                r#""level":"WARN""#,
                r#""program":"COTRN01C""#,
                r#""paragraph":"READ-TRANSACT-FILE""#,
                cause.as_str(),
            ] {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
        }

        traces.clear();
        let record = first_fixture_record()?;
        let found = Ok(Some(record.clone()));
        assert_eq!(lookup(&found), TranLookup::Found(&record));
        assert_eq!(lookup(&Ok(None)), TranLookup::NotFound);
        assert!(traces.text().is_empty(), "{}", traces.text());
        Ok(())
    }
}
