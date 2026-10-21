//! COTRTLIC, the transaction-type list (transaction `CTLI`, map `CTRTLIA`): the service of
//! endpoint 24, `GET /api/v1/transaction-types` (the screen load and F7/F8), and endpoint 25,
//! `POST /api/v1/transaction-types/list-actions` (ENTER and F10), both behind the admin gate
//! (COTRTLIC-001; AAP 0.3.4.1).
//!
//! It reads and writes the Db2 data version of the types, `db2.transaction_type`, through
//! [`Db2TransactionTypeRepository`] only, never the VSAM version that `tranrept` reads (E23).
//!
//! # Shape
//!
//! Every rule of the program is `domain::online::cotrtlic`'s: the edits in COTRTLIC's order
//! (COTRTLIC-019 to COTRTLIC-034), the F10 downgrade to ENTER (COTRTLIC-036 to COTRTLIC-038), the
//! E28 selection handling (COTRTLIC-041), the prompts, the messages and the dispatch of
//! `0000-MAIN` (`COTRTLIC.cbl:657-880`). The domain runs a request as a state machine and names,
//! at each step, the one store call the COBOL makes there; this service runs that call and hands
//! its result back:
//!
//! - `Count`: `9100-CHECK-FILTERS`, [`Db2TransactionTypeRepository::count_matching`]
//!   (COTRTLIC-031, COTRTLIC-032);
//! - `Verify` and `Commit`: the re-read of the row an F10 targets, then `9200-UPDATE-RECORD` or
//!   `9300-DELETE-RECORD`, in one transaction (COTRTLIC-042 to COTRTLIC-051);
//! - `Read`: `8000-READ-FORWARD` or `8100-READ-BACKWARDS`, [`page_after`] or [`page_before`],
//!   the last call of every request that reads (COTRTLIC-004, COTRTLIC-011, COTRTLIC-052).
//!
//! The handler extracts, calls one function here and maps the outcome; every branch on the key,
//! the state, the edits and the store results is here or in the domain (Thin Request Handlers).
//!
//! # State and trust
//!
//! `TranTypeListState` is COTRTLIC's COMMAREA, `WS-THIS-PROGCOMMAREA`, echoed by the client: in
//! the body of endpoint 25 and as the opaque `page_key` of endpoint 24. It is rebuilt through the
//! domain's total constructor and carries no authority: the acting identity is the verified
//! token, every request re-runs every edit, and every commit re-reads its row inside the
//! transaction that writes it and edits the description again against that row (AAP 0.5.1).
//!
//! # Store outcomes
//!
//! - A failed count or page read is the domain's `StoreFailure`, which it answers with its
//!   audited Db2 message, the page and the state (COTRTLIC-032, COTRTLIC-052).
//! - An update or delete outcome becomes the domain's `CommitResult`, the `SQLCODE` class
//!   `9200` and `9300` test: one row written is `SYNCPOINT`, here the commit; no row is +100; a
//!   lock failure on the update (`40P01`, `55P03`, `40001`) is -911; a delete refused by the
//!   categories' `ON DELETE RESTRICT` is -532 (`TRNTYCAT.ddl:6-7`); anything else is the
//!   program's `WHEN OTHER`. Every outcome but a write is rolled back.
//! - The text Db2 appends after ` SQLCODE:` is `HALTED(H3)`, and the domain leaves it empty
//!   (COTRTLIC-054, COTRTLIC-055).
//! - `9998-PRIMING-QUERY` has no repository counterpart and no call here: the replacement does
//!   not probe the database before the request's own calls, so a database it cannot reach fails
//!   the count or the read, answered as above (COTRTLIC-053).
//! - [`ServiceError::Store`], a 500 with no page, is kept for what no COBOL statement stands
//!   for: opening, committing or rolling back the commit transaction, and the re-read of the row
//!   an F10 targets, which the replacement adds.
//!
//! Each store failure the response answers without its cause is recorded as one tracing event
//! with the call and the error, whose text is an SQLSTATE and a constraint name only (Cardholder
//! Data Redaction; COTRTLIC handles no protected value).
//!
//! [`page_after`]: Db2TransactionTypeRepository::page_after
//! [`page_before`]: Db2TransactionTypeRepository::page_before

use domain::online::cotrtlic::{
    self, CommitEffect, CommitResult, ListActionOutcome, ListFailure, ListInput, PageSize,
    Progress, ReadDirection, ReadStep, RowInput, StoreFailure, TranTypeListPage,
};
use domain::records::Db2TransactionType;
use persistence::{Db2TransactionTypeRepository, PersistenceError, PgStore, PgTx};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cotrtlic::{TranTypeListActionRequest, TranTypeListQuery};
use crate::dto::resolve_page_size;
use crate::error::ServiceError;

/// The SQLSTATEs that stand for Db2's `SQLCODE` -911, a deadlock or lock timeout, on the
/// update of `9200-UPDATE-RECORD`: `deadlock_detected`, `lock_not_available` and
/// `serialization_failure` (COTRTLIC-047).
const LOCK_FAILURES: [&str; 3] = ["40P01", "55P03", "40001"];

/// Endpoint 24: the screen load, or F7 and F8 under the filters on the screen.
///
/// With `direction` and the `page_key` of the previous response it is F7 or F8 from the state
/// that key encodes, with the filter edits run as on ENTER (COTRTLIC-007 to COTRTLIC-013,
/// X-COTRTLIC-6). Otherwise it is the screen load, a first entry that reads page 1 under no
/// filter and ignores every other member (COTRTLIC-002, COTRTLIC-003).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the domain's [`ListFailure`] for a `page_key` that encodes no
/// state a COTRTLIC screen could hold. Every audited outcome, a store failure of the count or
/// the read included, is the returned page with its status class.
pub(crate) async fn list(
    state: &AppState,
    _admin: &AdminUser,
    query: TranTypeListQuery,
) -> Result<TranTypeListPage, ServiceError<ListFailure>> {
    list_on(&state.store, query).await
}

/// Endpoint 25: ENTER or F10 with the map input over the echoed state (COTRTLIC-006,
/// COTRTLIC-017 to COTRTLIC-051).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the domain's [`ListFailure`] for an echoed state no COTRTLIC
/// screen could hold; [`ServiceError::Store`] when the transaction of an F10 commit cannot be
/// opened, committed or rolled back, or its re-read of the target row fails. Every audited
/// outcome is the returned outcome with its status class.
pub(crate) async fn list_actions(
    state: &AppState,
    _admin: &AdminUser,
    request: TranTypeListActionRequest,
) -> Result<ListActionOutcome, ServiceError<ListFailure>> {
    act_on(&state.store, request).await
}

/// [`list`] over `store`.
async fn list_on(
    store: &PgStore,
    query: TranTypeListQuery,
) -> Result<TranTypeListPage, ServiceError<ListFailure>> {
    // The screen load ignores the key, so only a page move decodes it.
    let echoed = if query.direction.is_some() {
        query.state().map_err(ServiceError::Rule)?
    } else {
        None
    };
    let TranTypeListQuery {
        type_filter,
        desc_filter,
        direction,
        page_size: requested,
        ..
    } = query;
    let size = page_size(requested);
    let progress = match (direction, echoed) {
        (Some(direction), Some(echoed)) => cotrtlic::page(
            direction.into(),
            type_filter.as_deref().unwrap_or_default(),
            desc_filter.as_deref().unwrap_or_default(),
            &echoed,
            size,
        ),
        _ => cotrtlic::load(size),
    };
    drive(store, progress).await
}

/// [`list_actions`] over `store`.
async fn act_on(
    store: &PgStore,
    request: TranTypeListActionRequest,
) -> Result<ListActionOutcome, ServiceError<ListFailure>> {
    let TranTypeListActionRequest {
        aid,
        type_filter,
        desc_filter,
        rows,
        state,
        page_size: requested,
    } = request;
    let received = cotrtlic::TranTypeListState::try_from(&state).map_err(ServiceError::Rule)?;
    let row_inputs: Vec<RowInput<'_>> = rows.iter().map(RowInput::from).collect();
    let input = ListInput {
        type_filter: &type_filter,
        desc_filter: &desc_filter,
        rows: &row_inputs,
    };
    let progress = cotrtlic::act(aid.into(), &input, &received, page_size(requested));
    drive(store, progress).await
}

/// The rows a page holds: `requested`, 7 when absent (Bounded Pagination). The domain serves at
/// most the seven positions of the map whatever the size.
fn page_size(requested: Option<i64>) -> PageSize {
    // TRUNCATION: checked, never lossy: `DEFAULT_PAGE_SIZE` is 7, which `u32` holds, and the
    // fallback is only reached by a value no `usize` of this program takes.
    let default = u32::try_from(cotrtlic::DEFAULT_PAGE_SIZE).unwrap_or(u32::MAX);
    PageSize::from_u32(resolve_page_size(requested, default)).unwrap_or_default()
}

/// Runs every store call `progress` names until the domain gives the response.
async fn drive(
    store: &PgStore,
    mut progress: Progress,
) -> Result<ListActionOutcome, ServiceError<ListFailure>> {
    let mut reader = store.clone();
    loop {
        progress = match progress {
            Progress::Count(step) => {
                let count = {
                    let query = step.query();
                    Db2TransactionTypeRepository::count_matching(
                        &mut reader,
                        query.type_filter(),
                        query.desc_pattern(),
                    )
                    .await
                };
                step.finish(count.map_err(|error| store_failure("count_matching", &error)))
            }
            commit @ (Progress::Verify(_) | Progress::Commit(_)) => {
                commit_effect(store, commit).await?
            }
            Progress::Read(step) => return Ok(read_page(&mut reader, step).await),
            Progress::Done(outcome) => return Ok(*outcome),
        };
    }
}

/// The page read of `step`: forward from its start key, ascending, or backward below it,
/// descending, under its filters and with its limit, the look-ahead row of a forward read
/// included (COTRTLIC-004, COTRTLIC-005, COTRTLIC-011). The rows go to the domain as returned.
async fn read_page(store: &mut PgStore, step: ReadStep) -> ListActionOutcome {
    let rows = {
        let query = step.query();
        let filters = query.filters();
        match query.direction() {
            ReadDirection::Forward => {
                Db2TransactionTypeRepository::page_after(
                    store,
                    query.start(),
                    filters.type_filter(),
                    filters.desc_pattern(),
                    query.limit(),
                )
                .await
            }
            ReadDirection::Backward => {
                Db2TransactionTypeRepository::page_before(
                    store,
                    query.start(),
                    filters.type_filter(),
                    filters.desc_pattern(),
                    query.limit(),
                )
                .await
            }
        }
    };
    step.finish(rows.map_err(|error| store_failure("page_read", &error)))
}

/// An F10 commit, from its `Verify` or `Commit` step, in one transaction: the target row is
/// re-read and handed to the domain, which edits the request again against it, then the write
/// the domain names runs and its outcome goes back to the domain (COTRTLIC-042 to
/// COTRTLIC-051).
///
/// The transaction commits only when the write changed one row, the COBOL's `SYNCPOINT`; it is
/// rolled back for every other outcome, and when the re-read leaves nothing to write. The step
/// returned is the next one outside the transaction: the page re-read after an update, or the
/// response after a delete, which re-reads nothing (COTRTLIC-045, COTRTLIC-049).
///
/// # Errors
///
/// [`ServiceError::Store`] when the transaction cannot be opened, committed or rolled back, or
/// the re-read fails; the transaction is then discarded with nothing written.
async fn commit_effect(
    store: &PgStore,
    mut progress: Progress,
) -> Result<Progress, ServiceError<ListFailure>> {
    let mut tx = store.begin().await?;
    loop {
        progress = match progress {
            Progress::Verify(step) => {
                let current =
                    Db2TransactionTypeRepository::find_by_id(&mut tx, step.code()).await?;
                step.finish(current.as_ref())
            }
            Progress::Commit(step) => {
                let result = write(&mut tx, step.effect()).await;
                if result == CommitResult::Done {
                    tx.commit().await?;
                } else {
                    tx.rollback().await?;
                }
                return Ok(step.finish(result));
            }
            next => {
                tx.rollback().await?;
                return Ok(next);
            }
        };
    }
}

/// The write of `effect` on `tx`, as the `SQLCODE` class the COBOL tests.
async fn write(tx: &mut PgTx<'_>, effect: &CommitEffect) -> CommitResult {
    match effect {
        // `9200-UPDATE-RECORD`: the description the domain trimmed and padded to 50 bytes
        // (COTRTLIC-044).
        CommitEffect::Update { code, description } => {
            match Db2TransactionType::new(code.clone(), description.clone()) {
                Ok(row) => update_result(Db2TransactionTypeRepository::update(tx, &row).await),
                // The domain hands over at most 50 bytes, which the constructor accepts; a
                // longer text is never written, as a failed update.
                Err(_) => CommitResult::Failed,
            }
        }
        // `9300-DELETE-RECORD` (COTRTLIC-049).
        CommitEffect::Delete { code } => {
            delete_result(Db2TransactionTypeRepository::delete(tx, code).await)
        }
    }
}

/// `9200-UPDATE-RECORD`'s `EVALUATE SQLCODE`: 0, +100, -911 or below 0 (COTRTLIC-045 to
/// COTRTLIC-048).
fn update_result(updated: Result<bool, PersistenceError>) -> CommitResult {
    match updated {
        Ok(true) => CommitResult::Done,
        Ok(false) => CommitResult::NotFound,
        Err(error) => {
            report("update", &error);
            if is_lock_failure(&error) {
                CommitResult::Deadlock
            } else {
                CommitResult::Failed
            }
        }
    }
}

/// `9300-DELETE-RECORD`'s `EVALUATE SQLCODE`: 0, -532 or `WHEN OTHER`, +100 included
/// (COTRTLIC-049 to COTRTLIC-051).
fn delete_result(deleted: Result<bool, PersistenceError>) -> CommitResult {
    match deleted {
        Ok(true) => CommitResult::Done,
        Ok(false) => CommitResult::NotFound,
        Err(error) if error.is_restrict() => CommitResult::Restricted,
        Err(error) => {
            report("delete", &error);
            CommitResult::Failed
        }
    }
}

/// Whether `error` is Db2's -911: a deadlock, a lock timeout or a serialization failure.
fn is_lock_failure(error: &PersistenceError) -> bool {
    error
        .sqlstate()
        .is_some_and(|sqlstate| LOCK_FAILURES.contains(&sqlstate))
}

/// A failed count or page read as the domain's [`StoreFailure`], reported first.
fn store_failure(call: &'static str, error: &PersistenceError) -> StoreFailure {
    report(call, error);
    StoreFailure
}

/// One event for a store failure whose cause the response does not carry. The error's text is
/// its SQLSTATE and constraint name, never a stored or request value.
fn report(call: &'static str, error: &PersistenceError) {
    tracing::error!(call, %error, "COTRTLIC store call failed");
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::http::StatusCode;
    use domain::copybook::PicX;
    use domain::online::cotrtlic::{CommitResult, PageSize};
    use domain::records::Db2TransactionType;
    use persistence::{Db2TransactionTypeRepository, PersistenceError, PgStore};
    use test_support::postgres::{PostgresGuard, start_postgres};

    use super::{act_on, delete_result, list_on, page_size, update_result};
    use crate::dto::PageDirection;
    use crate::dto::cotrtlic::{
        Aid, TranTypeListActionRequest, TranTypeListActionResult, TranTypeListCommitted,
        TranTypeListPendingAction, TranTypeListQuery, TranTypeListResponse, TranTypeListRowInput,
        TranTypeListState,
    };
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A migrated database holding the Db2 seed of migration 0006, types `01` to `07`, each with
    /// categories, plus `extra` types without categories.
    async fn database(
        extra: &[(&str, &str)],
    ) -> Result<(PostgresGuard, PgStore), Box<dyn StdError>> {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        for &(code, description) in extra {
            let row = Db2TransactionType::new(PicX::from_text(code)?, description.to_owned())?;
            Db2TransactionTypeRepository::insert(&mut store, &row).await?;
        }
        Ok((guard, store))
    }

    /// The stored description of `code`, or `None` when no row has it.
    async fn stored(store: &PgStore, code: &str) -> Result<Option<String>, Box<dyn StdError>> {
        let mut store = store.clone();
        let row =
            Db2TransactionTypeRepository::find_by_id(&mut store, &PicX::from_text(code)?).await?;
        Ok(row.map(|row| row.tr_description().to_owned()))
    }

    /// The screen load of endpoint 24.
    async fn load(store: &PgStore) -> Result<TranTypeListResponse, Box<dyn StdError>> {
        Ok(list_on(store, TranTypeListQuery::default()).await?.into())
    }

    /// F7 or F8 of endpoint 24 from `from`, with blank filters.
    async fn page_move(
        store: &PgStore,
        from: &TranTypeListResponse,
        direction: PageDirection,
    ) -> Result<TranTypeListResponse, Box<dyn StdError>> {
        let query = TranTypeListQuery {
            page_key: Some(from.page_key.clone()),
            direction: Some(direction),
            ..TranTypeListQuery::default()
        };
        Ok(list_on(store, query).await?.into())
    }

    /// An endpoint 25 body over `state`: the filters, then `(row number, selection,
    /// description)` for the rows that are not blank.
    fn request(
        aid: Aid,
        state: &TranTypeListState,
        filters: (&str, &str),
        entries: &[(usize, &str, &str)],
    ) -> TranTypeListActionRequest {
        let rows = (1..=7)
            .map(|number| {
                entries
                    .iter()
                    .find(|(wanted, ..)| *wanted == number)
                    .map_or_else(TranTypeListRowInput::default, |&(_, selection, text)| {
                        TranTypeListRowInput {
                            selection: selection.to_owned(),
                            description: text.to_owned(),
                        }
                    })
            })
            .collect();
        TranTypeListActionRequest {
            aid,
            type_filter: filters.0.to_owned(),
            desc_filter: filters.1.to_owned(),
            rows,
            state: state.clone(),
            page_size: None,
        }
    }

    /// Endpoint 25 over `store`.
    async fn act(
        store: &PgStore,
        body: TranTypeListActionRequest,
    ) -> Result<TranTypeListActionResult, Box<dyn StdError>> {
        Ok(act_on(store, body).await?.into())
    }

    /// The codes of the seven positions shown, empty for an empty position.
    fn codes<T>(rows: &[Option<T>], code: fn(&T) -> &str) -> Vec<String> {
        rows.iter()
            .map(|row| row.as_ref().map(code).unwrap_or_default().to_owned())
            .collect()
    }

    fn listed(codes_shown: [&str; 7]) -> Vec<String> {
        codes_shown.map(str::to_owned).to_vec()
    }

    // audit: COTRTLIC-047
    // audit: COTRTLIC-048
    // audit: COTRTLIC-049
    // audit: COTRTLIC-050
    // audit: COTRTLIC-051
    #[test]
    fn writes_are_classified_as_the_sqlcode_classes_9200_and_9300_test() {
        let database = |sqlstate: &str| PersistenceError::Database {
            sqlstate: Some(sqlstate.to_owned()),
            constraint: None,
        };
        let restrict = PersistenceError::Restrict {
            constraint: Some("trc_type_code".to_owned()),
        };
        let cases = [
            (Ok(true), CommitResult::Done, CommitResult::Done),
            (Ok(false), CommitResult::NotFound, CommitResult::NotFound),
            (
                Err(database("40P01")),
                CommitResult::Deadlock,
                CommitResult::Failed,
            ),
            (
                Err(database("55P03")),
                CommitResult::Deadlock,
                CommitResult::Failed,
            ),
            (
                Err(database("40001")),
                CommitResult::Deadlock,
                CommitResult::Failed,
            ),
            (
                Err(database("57014")),
                CommitResult::Failed,
                CommitResult::Failed,
            ),
            (
                Err(PersistenceError::Unavailable),
                CommitResult::Failed,
                CommitResult::Failed,
            ),
            (
                Err(restrict),
                CommitResult::Failed,
                CommitResult::Restricted,
            ),
        ];
        for (written, update, delete) in cases {
            assert_eq!(update_result(written.clone()), update);
            assert_eq!(delete_result(written), delete);
        }
    }

    #[test]
    fn a_page_holds_the_requested_rows_seven_by_default_and_at_most() {
        for (requested, rows) in [
            (None, 7),
            (Some(1), 1),
            (Some(3), 3),
            (Some(7), 7),
            (Some(100), 7),
            (Some(0), 7),
            (Some(-1), 7),
        ] {
            assert_eq!(page_size(requested).rows(), rows);
        }
        assert_eq!(page_size(None), PageSize::DEFAULT);
    }

    // audit: COTRTLIC-002
    // audit: COTRTLIC-004
    // audit: COTRTLIC-007
    // audit: COTRTLIC-011
    #[tokio::test]
    async fn the_load_and_the_page_moves_read_the_db2_version_by_key() -> TestResult {
        let (_guard, store) =
            database(&[("08", "FEE"), ("09", "INTEREST"), ("10", "CASHBACK")]).await?;

        let loaded = load(&store).await?;
        assert_eq!(loaded.status(), StatusCode::OK);
        assert_eq!(
            codes(&loaded.rows, |row| row.type_code.as_str()),
            listed(["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!((loaded.page_no, loaded.has_next_page), (1, true));
        assert_eq!(
            loaded.info_message.as_deref(),
            Some("Type U to update, D to delete any record")
        );

        let forward = page_move(&store, &loaded, PageDirection::Forward).await?;
        assert_eq!(forward.status(), StatusCode::OK);
        assert_eq!(
            codes(&forward.rows, |row| row.type_code.as_str()),
            listed(["08", "09", "10", "", "", "", ""])
        );
        assert_eq!((forward.page_no, forward.has_next_page), (2, false));
        assert_eq!(
            forward.message.as_deref(),
            Some("No more pages for these search conditions")
        );

        let backward = page_move(&store, &forward, PageDirection::Backward).await?;
        assert_eq!(backward.status(), StatusCode::OK);
        assert_eq!(
            codes(&backward.rows, |row| row.type_code.as_str()),
            listed(["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!((backward.page_no, backward.has_next_page), (1, true));
        Ok(())
    }

    // audit: COTRTLIC-005
    // audit: COTRTLIC-029
    // audit: COTRTLIC-030
    // audit: COTRTLIC-031
    #[tokio::test]
    async fn valid_filters_are_counted_then_read_and_a_filter_matching_nothing_is_an_input_error()
    -> TestResult {
        let (_guard, store) = database(&[]).await?;
        let state = load(&store).await?.state;

        let by_type = act(&store, request(Aid::Enter, &state, ("03", ""), &[])).await?;
        assert_eq!(by_type.status(), StatusCode::OK);
        assert_eq!(
            codes(&by_type.rows, |row| row.type_code.as_str()),
            listed(["03", "", "", "", "", "", ""])
        );
        assert_eq!(by_type.state.type_filter, "03");

        let by_description = act(&store, request(Aid::Enter, &state, ("", "PAY"), &[])).await?;
        assert_eq!(by_description.status(), StatusCode::OK);
        assert_eq!(
            codes(&by_description.rows, |row| row.type_code.as_str()),
            listed(["02", "", "", "", "", "", ""])
        );

        let nothing = act(&store, request(Aid::Enter, &state, ("", "NOTHING"), &[])).await?;
        assert_eq!(nothing.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            nothing.message.as_deref(),
            Some("No Records found for these filter conditions")
        );
        Ok(())
    }

    // audit: COTRTLIC-036
    // audit: COTRTLIC-039
    // audit: COTRTLIC-042
    // audit: COTRTLIC-049
    // audit: COTRTLIC-050
    #[tokio::test]
    async fn f10_deletes_a_type_without_categories_and_restrict_keeps_one_with_them() -> TestResult
    {
        let (_guard, store) = database(&[("08", "OBSOLETE")]).await?;
        let state = load(&store).await?.state;

        let prompted = act(
            &store,
            request(Aid::Enter, &state, ("", ""), &[(2, "D", "")]),
        )
        .await?;
        assert_eq!(
            (prompted.state.pending_action, prompted.state.selected_row),
            (TranTypeListPendingAction::Delete, Some(2))
        );
        let restricted = act(
            &store,
            request(Aid::Pf10, &prompted.state, ("", ""), &[(2, "D", "")]),
        )
        .await?;
        assert_eq!(restricted.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            restricted.message.as_deref(),
            Some("Please delete associated child records first: SQLCODE:")
        );
        assert_eq!(restricted.committed, TranTypeListCommitted::Nothing);
        assert_eq!(
            restricted.state.pending_action,
            TranTypeListPendingAction::Delete
        );
        assert_eq!(stored(&store, "02").await?.as_deref(), Some("PAYMENT"));

        let filtered = act(&store, request(Aid::Enter, &state, ("08", ""), &[])).await?;
        assert_eq!(
            codes(&filtered.rows, |row| row.type_code.as_str()),
            listed(["08", "", "", "", "", "", ""])
        );
        let prompted = act(
            &store,
            request(Aid::Enter, &filtered.state, ("08", ""), &[(1, "D", "")]),
        )
        .await?;
        assert_eq!(
            prompted.info_message.as_deref(),
            Some("Delete HIGHLIGHTED row ? Press F10 to confirm")
        );
        let deleted = act(
            &store,
            request(Aid::Pf10, &prompted.state, ("08", ""), &[(1, "D", "")]),
        )
        .await?;
        assert_eq!(deleted.status(), StatusCode::OK);
        assert_eq!(deleted.committed, TranTypeListCommitted::Deleted);
        assert_eq!(
            deleted.info_message.as_deref(),
            Some("HIGHLIGHTED row deleted.Hit Enter to continue")
        );
        assert_eq!(stored(&store, "08").await?, None);
        assert_eq!(
            (deleted.state.pending_action, deleted.state.page_no),
            (TranTypeListPendingAction::None, 1)
        );
        Ok(())
    }

    // audit: COTRTLIC-040
    // audit: COTRTLIC-044
    // audit: COTRTLIC-045
    // audit: COTRTLIC-046
    #[tokio::test]
    async fn f10_writes_the_trimmed_description_and_a_row_deleted_by_others_is_not_found()
    -> TestResult {
        let (_guard, mut store) = database(&[("00", "TEMPORARY")]).await?;
        let state = load(&store).await?.state;

        let opened = act(
            &store,
            request(Aid::Enter, &state, ("", ""), &[(2, "U", "")]),
        )
        .await?;
        assert_eq!(opened.state.description_entry, Some(2));
        let edited = act(
            &store,
            request(
                Aid::Enter,
                &opened.state,
                ("", ""),
                &[(2, "U", "Bought goods")],
            ),
        )
        .await?;
        assert_eq!(edited.status(), StatusCode::OK);
        let saved = act(
            &store,
            request(
                Aid::Pf10,
                &edited.state,
                ("", ""),
                &[(2, "U", "Bought goods")],
            ),
        )
        .await?;
        assert_eq!(saved.status(), StatusCode::OK);
        assert_eq!(saved.committed, TranTypeListCommitted::Updated);
        assert_eq!(
            saved.info_message.as_deref(),
            Some("HIGHLIGHTED row was updated")
        );
        assert_eq!(
            stored(&store, "01").await?,
            Some(format!("{:<50}", "Bought goods"))
        );

        let opened = act(
            &store,
            request(Aid::Enter, &state, ("", ""), &[(1, "U", "")]),
        )
        .await?;
        let edited = act(
            &store,
            request(Aid::Enter, &opened.state, ("", ""), &[(1, "U", "Kept")]),
        )
        .await?;
        assert!(Db2TransactionTypeRepository::delete(&mut store, &PicX::from_text("00")?).await?);
        let gone = act(
            &store,
            request(Aid::Pf10, &edited.state, ("", ""), &[(1, "U", "Kept")]),
        )
        .await?;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            gone.message.as_deref(),
            Some("Record not found. Deleted by others ? SQLCODE:")
        );
        assert_eq!(gone.committed, TranTypeListCommitted::Nothing);
        assert_eq!(gone.state.pending_action, TranTypeListPendingAction::Update);
        Ok(())
    }

    // audit: COTRTLIC-052
    #[tokio::test]
    async fn a_database_the_service_cannot_reach_fails_the_read_or_the_commit() -> TestResult {
        let (guard, store) = database(&[]).await?;
        let state = load(&store).await?.state;
        let prompted = act(
            &store,
            request(Aid::Enter, &state, ("", ""), &[(2, "D", "")]),
        )
        .await?;
        guard.pool().close().await;

        let unread = load(&store).await?;
        assert_eq!(unread.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            unread.message.as_deref(),
            Some("C-TR-TYPE-FORWARD Open SQLCODE:")
        );

        let uncommitted = act_on(
            &store,
            request(Aid::Pf10, &prompted.state, ("", ""), &[(2, "D", "")]),
        )
        .await;
        assert_eq!(
            uncommitted.map(drop),
            Err(ServiceError::Store(PersistenceError::Unavailable))
        );
        Ok(())
    }
}
