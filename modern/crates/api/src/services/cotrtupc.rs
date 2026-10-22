//! COTRTUPC, transaction-type maintenance: the services of endpoints 26 and 27,
//! `GET /api/v1/transaction-types/{type_code}` and
//! `POST /api/v1/transaction-types/{type_code}/actions` (transaction `CTTU`, map `CTRTUPA`, admin
//! only; AAP 0.3.4.1).
//!
//! This module is the port of COTRTUPC's SQL: the key read of `9100-GET-TRANSACTION-TYPE`, the
//! update of `9600-WRITE-PROCESSING`, the insert of `9700-INSERT-RECORD` and the delete of
//! `9800-DELETE-PROCESSING` (`COTRTUPC.cbl:1469-1665`). Every edit, state rule and message is
//! `domain::online::cotrtupc`'s: the service reads, runs the writes the domain asks for, reports
//! how the store ended them, and commits (Thin Request Handlers).
//!
//! # Data version (E23)
//!
//! Only `db2.transaction_type` is read or written, through [`Db2TransactionTypeRepository`]
//! (COTRTUPC-002). `db2.transaction_type_category` is reached only through the `ON DELETE
//! RESTRICT` of its foreign key (`TRNTYCAT.ddl:5-7`; COTRTUPC-033), and the VSAM data version
//! that `tranrept` reads is never touched, so a CTTU change never reaches the report lookups.
//!
//! # Endpoint 26, the fetch
//!
//! [`fetch`] edits `{type_code}` with the key edits (COTRTUPC-004 to COTRTUPC-008), reads that
//! key and answers with the domain's view: state S with the row (COTRTUPC-009), state X with the
//! add prompt when no row holds it (COTRTUPC-010), or the 500 of a failed read (COTRTUPC-011). A
//! code the edits refuse is read from nowhere.
//!
//! The request carries no marker of state X, so the key edits run on every fetch, also for the
//! ENTER the screen sends in state X, where the COBOL skips them (COTRTUPC-012,
//! X-COTRTUPC-1, awaiting the reviewer's choice of mechanism).
//!
//! # Endpoint 27, the actions
//!
//! [`act`] serves a request in the program's order:
//!
//! 1. A key the endpoint does not serve is answered `Invalid key pressed` before anything is
//!    stored, edited or read (COTRTUPC-043).
//! 2. The received code is stored as `1150-STORE-MAP-IN-NEW` stores it, the blank test included
//!    (COTRTUPC-004, COTRTUPC-005), and the row it names is re-read in place of
//!    `TTUP-OLD-DETAILS`, the fetched values the COMMAREA kept (X-COTRTUPC-2).
//! 3. The state is derived on the server from the key and that row, never taken from the
//!    request ([`cotrtupc::served_state`]); [`cotrtupc::decide`] gives the step's outcome
//!    (COTRTUPC-016 to COTRTUPC-024, COTRTUPC-032, COTRTUPC-038).
//! 4. A write the domain asks for runs inside one transaction, and its result goes back to
//!    [`cotrtupc::apply_result`]. An update that changed no row is followed, in the same
//!    transaction, by the insert `apply_result` returns (COTRTUPC-026). The transaction commits
//!    after a statement that completed, as the program issues `EXEC CICS SYNCPOINT`
//!    (COTRTUPC-024, COTRTUPC-026, COTRTUPC-032), and rolls back after any other result, which
//!    leaves the table as it was (COTRTUPC-027 to COTRTUPC-029, COTRTUPC-033, COTRTUPC-034).
//!
//! The re-read and every write follow from the request alone. The screen's state is no input,
//! and the received code and description are edited again before a save (COTRTUPC-024), so
//! nothing a client echoes authorises a write (AAP 0.5.1).
//!
//! # Store outcomes
//!
//! | Repository result | Db2 `SQLCODE` the program tests | [`StoreResult`] |
//! |---|---|---|
//! | `update` or `delete` `Ok(true)`, `insert` `Ok(())` | 0 | [`StoreResult::Done`] |
//! | `update` or `delete` `Ok(false)` | +100 | [`StoreResult::NoRow`] |
//! | SQLSTATE `40P01` or `55P03` | -911 | [`StoreResult::LockNotAvailable`] |
//! | [`PersistenceError::Restrict`] (`23503`) | -532 | [`StoreResult::ForeignKeyViolation`] |
//! | every other error | any other negative code | [`StoreResult::OtherError`] |
//!
//! The SQLSTATE mapping is the domain's ([`StoreResult::from_sqlstate`]); the -911 states are
//! those of COTRTUPC-029. A failed key read is [`TypeLookup::Error`]. Only `begin`, `commit` and
//! `rollback` of the transaction, which no COBOL statement words, fail as
//! [`ServiceError::Store`].
//!
//! # Halts
//!
//! `HALTED(H3)`: a store failure answers with the program's fixed message prefix only. The
//! edited `SQLCODE` and `SQLERRM` that the COBOL strings after it, twice in the delete-restricted
//! message (E19), are Db2's text and are neither produced nor passed on: no response carries an
//! SQLSTATE, a constraint name or a stored value (COTRTUPC-049).
//!
//! # Redaction
//!
//! No value this program handles is protected. A store error is logged as its value-free
//! `Display`, which names at most an SQLSTATE, a constraint and a column (Cardholder Data
//! Redaction).

use domain::copybook::PicX;
use domain::online::cotrtupc::{
    self, MaintEffect, MaintFailure, MaintOutcome, StoreResult, TranTypeView, TtupAid, TtupState,
    TypeLookup,
};
use domain::records::Db2TransactionType;
use persistence::{Db2TransactionTypeRepository, PersistenceError, PgStore};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cotrtupc::TranTypeActionRequest;
use crate::error::ServiceError;

/// The result of a key read: the row, no row, or the store error.
type KeyRead = Result<Option<Db2TransactionType>, PersistenceError>;

/// Endpoint 26: ENTER on the fresh screen, the fetch of the type `type_code` names
/// (COTRTUPC-004 to COTRTUPC-011).
///
/// The admin gate has passed: an [`AdminUser`] exists only for a verified token of user type `A`
/// (COTRTUPC-001).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with status class `Input` and field `type_code` when the key edits
///   refuse the code: `Tran Type code must be supplied.`, `Tran Type code must be numeric.` or
///   `Tran Type code must not be zero.` (COTRTUPC-005 to COTRTUPC-007). Nothing is read.
/// - [`ServiceError::Rule`] with status class `Store` and `Error accessing: TRANSACTION_TYPE
///   table. SQLCODE:` when the read fails, the rest `HALTED(H3)` (COTRTUPC-011, COTRTUPC-049).
pub(crate) async fn fetch(
    state: &AppState,
    _admin: &AdminUser,
    type_code: &str,
) -> Result<TranTypeView, ServiceError<MaintFailure>> {
    show(&state.store, type_code).await
}

/// Endpoint 27: ENTER (validate), PF5 (save) or PF4 (the confirmed delete) for the type
/// `type_code` names, with the description `request` carries (COTRTUPC-016 to COTRTUPC-034,
/// COTRTUPC-038, COTRTUPC-043).
///
/// Returns the screen of the step: state N after a validated change, state C after a committed
/// save, state 7 after a committed delete, or the abend data of a step no branch of
/// `2000-DECIDE-ACTION` serves (COTRTUPC-037). The admin gate has passed (COTRTUPC-001).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the program's message, field and status class for every edit,
///   every key the endpoint does not serve and every store failure, nothing written:
///   `Invalid key pressed` (COTRTUPC-043), the key and description edits (COTRTUPC-005,
///   COTRTUPC-017 to COTRTUPC-019), the update, insert, lock and delete failures
///   (COTRTUPC-027 to COTRTUPC-029, COTRTUPC-033, COTRTUPC-034), and the blank 500 of a failed
///   re-read (COTRTUPC-038).
/// - [`ServiceError::Store`] when the transaction cannot begin, commit or roll back.
/// - [`ServiceError::Internal`] when a write the domain asks for does not fit the row type, which
///   the domain's edits rule out.
pub(crate) async fn act(
    state: &AppState,
    _admin: &AdminUser,
    type_code: &str,
    request: TranTypeActionRequest,
) -> Result<MaintOutcome, ServiceError<MaintFailure>> {
    perform(&state.store, type_code, request).await
}

/// [`fetch`] over `store`.
async fn show(
    store: &PgStore,
    type_code: &str,
) -> Result<TranTypeView, ServiceError<MaintFailure>> {
    // The key edits always run: the request has no state-X marker for `cotrtupc::fetch_key`
    // to skip them by (COTRTUPC-012, X-COTRTUPC-1).
    let key = cotrtupc::edit_type_code(type_code).map_err(ServiceError::Rule)?;
    let read = read_key(store, &key).await;
    cotrtupc::fetch(&key, lookup(&read)).map_err(ServiceError::Rule)
}

/// [`act`] over `store`.
async fn perform(
    store: &PgStore,
    type_code: &str,
    request: TranTypeActionRequest,
) -> Result<MaintOutcome, ServiceError<MaintFailure>> {
    let aid = TtupAid::from(request.aid);
    let input = request.input(type_code);
    if aid == TtupAid::Pf12 {
        // `decide` answers a key the endpoint does not serve with `Invalid key pressed`, never as
        // ENTER, before it consults the state or the row, so neither is derived and nothing is
        // read (COTRTUPC-042, COTRTUPC-043).
        return settle(cotrtupc::decide(
            aid,
            TtupState::DetailsNotFetched,
            &input,
            TypeLookup::NotFound,
        ));
    }
    let key = cotrtupc::receive_type_code(type_code).map_err(ServiceError::Rule)?;
    let read = read_key(store, &key).await;
    let current = lookup(&read);
    // The served state comes from the key and the re-read row; the request carries none, so no
    // echoed state can steer a write (X-COTRTUPC-2, AAP 0.5.1).
    let step = cotrtupc::decide(aid, cotrtupc::served_state(aid, current), &input, current);
    match step {
        MaintOutcome::Execute(effect) => settle(execute(store, effect).await?),
        outcome => settle(outcome),
    }
}

/// `9100-GET-TRANSACTION-TYPE`: the row `key` names, read on a pooled connection
/// (COTRTUPC-009 to COTRTUPC-011).
async fn read_key(store: &PgStore, key: &PicX<2>) -> KeyRead {
    let mut reader = store.clone();
    let read = Db2TransactionTypeRepository::find_by_id(&mut reader, key).await;
    if let Err(error) = &read {
        log_store_error("9100-GET-TRANSACTION-TYPE", error);
    }
    read
}

/// What a key read found, as the domain receives it.
fn lookup(read: &KeyRead) -> TypeLookup<'_> {
    match read {
        Ok(Some(row)) => TypeLookup::Found(row),
        Ok(None) => TypeLookup::NotFound,
        Err(_) => TypeLookup::Error,
    }
}

/// Runs `effect` and every write that follows it in one transaction, until the domain gives the
/// step's answer, then commits after a statement that completed and rolls back after any other
/// result (COTRTUPC-024 to COTRTUPC-029, COTRTUPC-032 to COTRTUPC-034).
///
/// An `UPDATE` that matches no row is no error, so the insert that follows it runs in the same,
/// still usable transaction (COTRTUPC-026). [`cotrtupc::apply_result`] asks for no further write
/// after an insert or a delete, so the loop runs at most twice.
async fn execute(
    store: &PgStore,
    effect: MaintEffect,
) -> Result<MaintOutcome, ServiceError<MaintFailure>> {
    let mut tx = store.begin().await?;
    let mut effect = effect;
    loop {
        let result = match run(&mut tx, &effect).await {
            Ok(result) => result,
            Err(error) => {
                tx.rollback().await?;
                return Err(error);
            }
        };
        match cotrtupc::apply_result(effect, result) {
            MaintOutcome::Execute(next) => effect = next,
            outcome => {
                if result == StoreResult::Done {
                    tx.commit().await?;
                } else {
                    tx.rollback().await?;
                }
                return Ok(outcome);
            }
        }
    }
}

/// Runs `effect` through `repo` and reports how the store ended it.
///
/// # Errors
///
/// [`ServiceError::Internal`] when the code and description of an update or insert do not fit
/// [`Db2TransactionType`]; nothing is run then.
async fn run<R: Db2TransactionTypeRepository>(
    repo: &mut R,
    effect: &MaintEffect,
) -> Result<StoreResult, ServiceError<MaintFailure>> {
    let result = match effect {
        MaintEffect::Update { code, description } => {
            let row = host_variables(code, description)?;
            changed(
                "9600-WRITE-PROCESSING",
                Db2TransactionTypeRepository::update(repo, &row).await,
            )
        }
        MaintEffect::Insert { code, description } => {
            let row = host_variables(code, description)?;
            match Db2TransactionTypeRepository::insert(repo, &row).await {
                Ok(()) => StoreResult::Done,
                Err(error) => failed("9700-INSERT-RECORD", &error),
            }
        }
        MaintEffect::Delete { code } => changed(
            "9800-DELETE-PROCESSING",
            Db2TransactionTypeRepository::delete(repo, code).await,
        ),
    };
    Ok(result)
}

/// `DCL-TR-TYPE` and `DCL-TR-DESCRIPTION` for a write: the code and the description the domain
/// edited, the description already trimmed and padded to 50 bytes (`COTRTUPC.cbl:1538-1542`;
/// COTRTUPC-023).
fn host_variables(
    code: &PicX<2>,
    description: &str,
) -> Result<Db2TransactionType, ServiceError<MaintFailure>> {
    Db2TransactionType::new(code.clone(), description.to_owned())
        .map_err(|_| ServiceError::Internal)
}

/// The result of an `UPDATE` or `DELETE`: whether it changed its row, or how it failed.
fn changed(paragraph: &'static str, outcome: Result<bool, PersistenceError>) -> StoreResult {
    match outcome {
        Ok(true) => StoreResult::Done,
        Ok(false) => StoreResult::NoRow,
        Err(error) => failed(paragraph, &error),
    }
}

/// The SQLCODE class a failed statement stands for, logged with the paragraph that ran it.
fn failed(paragraph: &'static str, error: &PersistenceError) -> StoreResult {
    log_store_error(paragraph, error);
    store_result(error)
}

/// The SQLCODE class of `error`: -532 for a refusing foreign key, whose error records no
/// SQLSTATE, otherwise the domain's reading of the SQLSTATE, and any other negative code when
/// there is none (COTRTUPC-029, COTRTUPC-033).
///
/// A serialization failure (`40001`) is no -911 here: COTRTUPC-029 names `40P01` and `55P03`
/// only, and [`StoreResult::from_sqlstate`] is the one owner of that mapping.
fn store_result(error: &PersistenceError) -> StoreResult {
    if error.is_restrict() {
        StoreResult::ForeignKeyViolation
    } else {
        error
            .sqlstate()
            .map_or(StoreResult::OtherError, StoreResult::from_sqlstate)
    }
}

/// Records a store error for the operator; the response carries only the program's message.
fn log_store_error(paragraph: &'static str, error: &PersistenceError) {
    tracing::error!(paragraph, error = %error, "COTRTUPC store call failed");
}

/// The service's answer to a step: a failure becomes the error, which carries its audited
/// status; every other outcome is the screen the handler sends.
fn settle(outcome: MaintOutcome) -> Result<MaintOutcome, ServiceError<MaintFailure>> {
    match outcome {
        MaintOutcome::Failed(failure) => Err(ServiceError::Rule(failure)),
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::copybook::PicX;
    use domain::online::cotrtupc::{
        self, FailureClass, MaintEffect, MaintFailure, MaintField, MaintOutcome, StoreResult,
        TranTypeView, TtupAid, TtupState, TypeLookup,
    };
    use domain::records::Db2TransactionType;
    use persistence::{
        Db2TransactionTypeCategoryRepository, Db2TransactionTypeRepository, PersistenceError,
        PgStore,
    };

    use super::{KeyRead, lookup, perform, run, settle, show, store_result};
    use crate::dto::cotrtupc::{Aid, TranTypeActionRequest};
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    // The audited literals, written out apart from the domain's constants.
    const SEARCH: &str = "Enter transaction type to be maintained";
    const NOT_FOUND: &str = "No record found for this key in database";
    const NO_CHANGE: &str = "No change detected with respect to values fetched.";
    const INVALID_KEY: &str = "Invalid key pressed";
    const CODE_SUPPLIED: &str = "Tran Type code must be supplied.";
    const CODE_NUMERIC: &str = "Tran Type code must be numeric.";
    const DESC_ALPHANUM: &str = "Transaction Desc can have numbers or alphabets only.";
    const VALIDATED: &str = "Changes validated.Press F5 to save";
    const COMMITTED: &str = "Changes committed to database";
    const DELETED: &str = "Delete successful.";
    const DELETE_RESTRICTED: &str = "Please delete associated child records first:SQLCODE :";
    const DELETE_FAILED: &str = "Delete failed with message:SQLCODE :";

    fn code(text: &str) -> Result<PicX<2>, Box<dyn StdError>> {
        Ok(PicX::<2>::from_text(text)?)
    }

    fn row(text: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            code(text)?,
            description.to_owned(),
        )?)
    }

    /// `text` space-padded to the 50 bytes of `TTUP-NEW-TTYP-TYPE-DESC`.
    fn padded(text: &str) -> String {
        format!("{text:<50}")
    }

    fn request(aid: Aid, description: Option<&str>) -> TranTypeActionRequest {
        TranTypeActionRequest {
            aid,
            description: description.map(str::to_owned),
        }
    }

    fn database(sqlstate: &str) -> PersistenceError {
        PersistenceError::Database {
            sqlstate: Some(sqlstate.to_owned()),
            constraint: None,
        }
    }

    /// The restricted delete of a type that still has categories (`SQLCODE` -532).
    fn restricted() -> PersistenceError {
        PersistenceError::Restrict {
            constraint: Some("trc_type_code".to_owned()),
        }
    }

    /// A screen as the response shows it: state, code, description, error row, info row.
    type Shown = (
        TtupState,
        String,
        Option<String>,
        Option<&'static str>,
        &'static str,
    );

    fn shown(view: &TranTypeView) -> Shown {
        (
            view.state(),
            view.code().to_text(),
            view.description().map(str::to_owned),
            view.message(),
            view.info_message(),
        )
    }

    fn screen(outcome: &MaintOutcome) -> Option<Shown> {
        match outcome {
            MaintOutcome::Screen(view) => Some(shown(view)),
            _ => None,
        }
    }

    /// The domain failure a service error carries, or `None` for any other error.
    fn rule<T>(answer: &Result<T, ServiceError<MaintFailure>>) -> Option<MaintFailure> {
        match answer {
            Err(ServiceError::Rule(failure)) => Some(*failure),
            _ => None,
        }
    }

    /// One repository call of [`Scripted`].
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Update(Db2TransactionType),
        Insert(Db2TransactionType),
        Delete(PicX<2>),
    }

    /// A repository double that answers each write with its scripted result and records the
    /// call, as a store-failure test needs (AAP 0.5.4).
    struct Scripted {
        update: Result<bool, PersistenceError>,
        insert: Result<(), PersistenceError>,
        delete: Result<bool, PersistenceError>,
        calls: Vec<Call>,
    }

    impl Scripted {
        fn answering(
            update: Result<bool, PersistenceError>,
            insert: Result<(), PersistenceError>,
            delete: Result<bool, PersistenceError>,
        ) -> Self {
            Self {
                update,
                insert,
                delete,
                calls: Vec::new(),
            }
        }
    }

    impl Db2TransactionTypeRepository for Scripted {
        async fn find_by_id(
            &mut self,
            _code: &PicX<2>,
        ) -> Result<Option<Db2TransactionType>, PersistenceError> {
            Ok(None)
        }

        async fn page_after(
            &mut self,
            _start: &PicX<2>,
            _type_filter: Option<&PicX<2>>,
            _desc_pattern: Option<&str>,
            _limit: i64,
        ) -> Result<Vec<Db2TransactionType>, PersistenceError> {
            Ok(Vec::new())
        }

        async fn page_before(
            &mut self,
            _start: &PicX<2>,
            _type_filter: Option<&PicX<2>>,
            _desc_pattern: Option<&str>,
            _limit: i64,
        ) -> Result<Vec<Db2TransactionType>, PersistenceError> {
            Ok(Vec::new())
        }

        async fn count_matching(
            &mut self,
            _type_filter: Option<&PicX<2>>,
            _desc_pattern: Option<&str>,
        ) -> Result<i64, PersistenceError> {
            Ok(0)
        }

        fn insert(
            &mut self,
            row: &Db2TransactionType,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.calls.push(Call::Insert(row.clone()));
            let answer = self.insert.clone();
            async move { answer }
        }

        fn update(
            &mut self,
            row: &Db2TransactionType,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            self.calls.push(Call::Update(row.clone()));
            let answer = self.update.clone();
            async move { answer }
        }

        fn delete(
            &mut self,
            code: &PicX<2>,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            self.calls.push(Call::Delete(code.clone()));
            let answer = self.delete.clone();
            async move { answer }
        }
    }

    // audit: COTRTUPC-029
    // audit: COTRTUPC-033
    #[test]
    fn store_errors_map_to_the_sqlcode_classes_the_program_tests() {
        for (error, expected) in [
            (database("40P01"), StoreResult::LockNotAvailable),
            (database("55P03"), StoreResult::LockNotAvailable),
            (restricted(), StoreResult::ForeignKeyViolation),
            (
                PersistenceError::Restrict { constraint: None },
                StoreResult::ForeignKeyViolation,
            ),
            (database("40001"), StoreResult::OtherError),
            (database("25P02"), StoreResult::OtherError),
            (
                PersistenceError::Duplicate {
                    constraint: Some("xtran_type".to_owned()),
                },
                StoreResult::OtherError,
            ),
            (PersistenceError::Unavailable, StoreResult::OtherError),
            (PersistenceError::NotFound, StoreResult::OtherError),
            (
                PersistenceError::Database {
                    sqlstate: None,
                    constraint: None,
                },
                StoreResult::OtherError,
            ),
        ] {
            assert_eq!(store_result(&error), expected, "{error:?}");
        }
    }

    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTUPC-011
    #[test]
    fn a_key_read_becomes_the_domain_lookup() -> TestResult {
        let stored = row("01", "PURCHASE")?;
        let found: KeyRead = Ok(Some(stored.clone()));
        assert_eq!(lookup(&found), TypeLookup::Found(&stored));
        let missing: KeyRead = Ok(None);
        assert_eq!(lookup(&missing), TypeLookup::NotFound);
        let failed: KeyRead = Err(PersistenceError::Unavailable);
        assert_eq!(lookup(&failed), TypeLookup::Error);
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    // audit: COTRTUPC-026
    // audit: COTRTUPC-027
    // audit: COTRTUPC-028
    // audit: COTRTUPC-029
    // audit: COTRTUPC-032
    // audit: COTRTUPC-033
    // audit: COTRTUPC-034
    #[tokio::test]
    async fn each_write_binds_the_domain_values_and_reports_its_result() -> TestResult {
        let description = padded("New type");
        let update = MaintEffect::Update {
            code: code("05")?,
            description: description.clone(),
        };
        let insert = MaintEffect::Insert {
            code: code("05")?,
            description: description.clone(),
        };
        let delete = MaintEffect::Delete { code: code("05")? };
        let written = row("05", &description)?;
        let cases = [
            (&update, Ok(true), Ok(()), Ok(true), StoreResult::Done),
            (&update, Ok(false), Ok(()), Ok(true), StoreResult::NoRow),
            (
                &update,
                Err(database("40P01")),
                Ok(()),
                Ok(true),
                StoreResult::LockNotAvailable,
            ),
            (
                &update,
                Err(database("55P03")),
                Ok(()),
                Ok(true),
                StoreResult::LockNotAvailable,
            ),
            (
                &update,
                Err(database("23502")),
                Ok(()),
                Ok(true),
                StoreResult::OtherError,
            ),
            (&insert, Ok(true), Ok(()), Ok(true), StoreResult::Done),
            (
                &insert,
                Ok(true),
                Err(PersistenceError::Duplicate {
                    constraint: Some("xtran_type".to_owned()),
                }),
                Ok(true),
                StoreResult::OtherError,
            ),
            (&delete, Ok(true), Ok(()), Ok(true), StoreResult::Done),
            (&delete, Ok(true), Ok(()), Ok(false), StoreResult::NoRow),
            (
                &delete,
                Ok(true),
                Ok(()),
                Err(restricted()),
                StoreResult::ForeignKeyViolation,
            ),
            (
                &delete,
                Ok(true),
                Ok(()),
                Err(PersistenceError::Unavailable),
                StoreResult::OtherError,
            ),
        ];
        for (effect, updated, inserted, deleted, expected) in cases {
            let mut repo = Scripted::answering(updated, inserted, deleted);
            assert_eq!(run(&mut repo, effect).await, Ok(expected), "{effect:?}");
            let call = match effect {
                MaintEffect::Update { .. } => Call::Update(written.clone()),
                MaintEffect::Insert { .. } => Call::Insert(written.clone()),
                MaintEffect::Delete { code } => Call::Delete(code.clone()),
            };
            assert_eq!(repo.calls, vec![call], "{effect:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_write_that_does_not_fit_the_row_runs_nothing() -> TestResult {
        let oversized = MaintEffect::Update {
            code: code("05")?,
            description: "X".repeat(51),
        };
        let mut repo = Scripted::answering(Ok(true), Ok(()), Ok(true));
        assert_eq!(
            run(&mut repo, &oversized).await,
            Err(ServiceError::Internal)
        );
        assert!(repo.calls.is_empty());
        Ok(())
    }

    #[test]
    fn a_failed_step_is_the_error_and_every_other_step_the_answer() -> TestResult {
        let stored = row("01", "PURCHASE")?;
        let failure = cotrtupc::check_pfkeys(TtupAid::Pf4, TtupState::DetailsNotFound)
            .err()
            .ok_or("PF4 accepted in state X")?;
        assert_eq!(
            settle(MaintOutcome::Failed(failure)),
            Err(ServiceError::Rule(failure))
        );
        let view = cotrtupc::fetch(&code("01")?, TypeLookup::Found(&stored))?;
        assert_eq!(
            settle(MaintOutcome::Screen(view.clone())),
            Ok(MaintOutcome::Screen(view))
        );
        Ok(())
    }

    /// A store over a freshly migrated container, which holds the Db2 seed of migration 0006 and
    /// nothing else: seven types, each with categories.
    async fn seeded() -> Result<(test_support::postgres::PostgresGuard, PgStore), Box<dyn StdError>>
    {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        Ok((guard, store))
    }

    /// The stored row of `text`, read through its own pooled connection.
    async fn stored(
        store: &PgStore,
        text: &str,
    ) -> Result<Option<Db2TransactionType>, Box<dyn StdError>> {
        let mut reader = store.clone();
        Ok(Db2TransactionTypeRepository::find_by_id(&mut reader, &code(text)?).await?)
    }

    /// Endpoint 26: the key edits normalise the code before the read, and refuse before it.
    async fn fetches(store: &PgStore) -> TestResult {
        let found = show(store, " 1").await?;
        assert_eq!(
            shown(&found),
            (
                TtupState::ShowDetails,
                "01".to_owned(),
                Some("PURCHASE".to_owned()),
                None,
                SEARCH
            )
        );
        let missing = show(store, "42").await?;
        assert_eq!(missing.state(), TtupState::DetailsNotFound);
        assert_eq!(missing.code(), &code("42")?);
        assert_eq!(missing.description(), None);
        assert_eq!(missing.message(), Some(NOT_FOUND));
        for (raw, message) in [("AB", CODE_NUMERIC), ("*", CODE_SUPPLIED)] {
            let refused = rule(&show(store, raw).await).ok_or("edit passed")?;
            assert_eq!(refused.message(), message, "{raw:?}");
            assert_eq!(refused.field(), Some(MaintField::TypeCode));
            assert_eq!(refused.class(), FailureClass::Input);
        }
        Ok(())
    }

    /// ENTER compares with the re-read row, edits the description and writes nothing.
    async fn validates(store: &PgStore) -> TestResult {
        let same = perform(store, "01", request(Aid::Enter, Some("purchase"))).await;
        assert_eq!(
            rule(&same).map(|failure| failure.message()),
            Some(NO_CHANGE)
        );
        let validated = perform(store, "01", request(Aid::Enter, Some(" Purchases 2 "))).await?;
        assert_eq!(
            screen(&validated),
            Some((
                TtupState::ChangesOkNotConfirmed,
                "01".to_owned(),
                Some(padded("Purchases 2")),
                None,
                VALIDATED
            ))
        );
        let refused = perform(store, "01", request(Aid::Enter, Some("Buy-now"))).await;
        assert_eq!(
            rule(&refused).map(|failure| (failure.message(), failure.field())),
            Some((DESC_ALPHANUM, Some(MaintField::Description)))
        );
        assert_eq!(stored(store, "01").await?, Some(row("01", "PURCHASE")?));
        Ok(())
    }

    /// PF5 updates a row that exists, its description trimmed and padded to 50 bytes; for a code
    /// no row holds, ENTER compares with blanks and PF5 inserts after the update that changed no
    /// row, in the same transaction.
    async fn saves(store: &PgStore) -> TestResult {
        let saved = perform(store, "01", request(Aid::Pf5, Some(" Purchases 2 "))).await?;
        assert_eq!(
            screen(&saved),
            Some((
                TtupState::ChangesOkayedAndDone,
                "01".to_owned(),
                Some(padded("Purchases 2")),
                None,
                COMMITTED
            ))
        );
        assert_eq!(
            stored(store, "01").await?,
            Some(row("01", &padded("Purchases 2"))?)
        );

        let new_type = perform(store, "42", request(Aid::Enter, Some("New type"))).await?;
        assert_eq!(
            screen(&new_type).map(|screen| screen.0),
            Some(TtupState::ChangesOkNotConfirmed)
        );
        let added = perform(store, "42", request(Aid::Pf5, Some("New type"))).await?;
        assert_eq!(
            screen(&added).map(|screen| (screen.0, screen.4)),
            Some((TtupState::ChangesOkayedAndDone, COMMITTED))
        );
        assert_eq!(
            stored(store, "42").await?,
            Some(row("42", &padded("New type"))?)
        );
        Ok(())
    }

    /// PF4 on a type with categories is refused by the foreign key and deletes nothing; on a type
    /// without categories it deletes, and a second delete finds no row.
    async fn deletes(store: &PgStore) -> TestResult {
        let mut reader = store.clone();
        let payment = code("02")?;
        let categories =
            Db2TransactionTypeCategoryRepository::find_all_by_type(&mut reader, &payment).await?;
        assert!(!categories.is_empty());
        let restricted = perform(store, "02", request(Aid::Pf4, None)).await;
        let refused = rule(&restricted).ok_or("restricted delete answered")?;
        assert_eq!(
            (refused.message(), refused.class(), refused.state()),
            (
                DELETE_RESTRICTED,
                FailureClass::Store,
                TtupState::StartDelete
            )
        );
        assert_eq!(stored(store, "02").await?, Some(row("02", "PAYMENT")?));
        assert_eq!(
            Db2TransactionTypeCategoryRepository::find_all_by_type(&mut reader, &payment).await?,
            categories
        );

        let deleted = perform(store, "42", request(Aid::Pf4, None)).await?;
        assert_eq!(
            screen(&deleted),
            Some((TtupState::DeleteDone, "42".to_owned(), None, None, DELETED))
        );
        assert_eq!(stored(store, "42").await?, None);
        let gone = perform(store, "42", request(Aid::Pf4, None)).await;
        assert_eq!(
            rule(&gone).map(|failure| (failure.message(), failure.state())),
            Some((DELETE_FAILED, TtupState::DeleteFailed))
        );
        Ok(())
    }

    /// A key the endpoint does not serve is answered before the code is edited or read; a blank
    /// code on a served key is refused by the blank test, and nothing is written.
    async fn refuses_unserved_keys_and_blank_codes(store: &PgStore) -> TestResult {
        for raw in ["", "01", "AB"] {
            let unserved = perform(store, raw, request(Aid::Unserved, Some("Other"))).await;
            let refused = rule(&unserved).ok_or("unserved key answered")?;
            assert_eq!((refused.message(), refused.field()), (INVALID_KEY, None));
        }
        let blank = perform(store, "  ", request(Aid::Pf5, Some("Other"))).await;
        assert_eq!(
            rule(&blank).map(|failure| failure.message()),
            Some(CODE_SUPPLIED)
        );
        assert_eq!(
            stored(store, "01").await?,
            Some(row("01", &padded("Purchases 2"))?)
        );
        Ok(())
    }

    // audit: COTRTUPC-002
    // audit: COTRTUPC-005
    // audit: COTRTUPC-006
    // audit: COTRTUPC-008
    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTUPC-017
    // audit: COTRTUPC-019
    // audit: COTRTUPC-020
    // audit: COTRTUPC-021
    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    // audit: COTRTUPC-026
    // audit: COTRTUPC-032
    // audit: COTRTUPC-033
    // audit: COTRTUPC-034
    // audit: COTRTUPC-043
    #[tokio::test]
    async fn the_maintenance_flow_reads_writes_and_commits_the_db2_version() -> TestResult {
        let (_guard, store) = seeded().await?;
        fetches(&store).await?;
        validates(&store).await?;
        saves(&store).await?;
        deletes(&store).await?;
        refuses_unserved_keys_and_blank_codes(&store).await
    }
}
