//! COCRDUPC, the card update (transaction `CCUP`, map `CCRDUPA`): the service of endpoint 9,
//! `GET /api/v1/cards/edit` (ENTER fetch), and endpoint 10, `PUT /api/v1/cards/{card_ref}`
//! (ENTER validate, F5 save), called only by `api::handlers::cocrdupc`.
//!
//! This module ports the program's file statements and nothing else: every edit, comparison,
//! decision and message is `domain::online::cocrdupc`'s (Thin Request Handlers).
//!
//! # Endpoint 9
//!
//! [`fetch`] runs the key edits over the raw query, the list entry included (COCRDUPC-002,
//! COCRDUPC-010 to COCRDUPC-014), then `9100-GETCARD-BYACCTCARD`: a `READ` of `CARDDAT` by the
//! card number alone, or by the `cards.card_ref` handle of a COCRDLI row in place of the number
//! COCRDLIC passes (`COCRDUPC.cbl:1376-1417`; COCRDUPC-016). A found card, no row (404,
//! COCRDUPC-003, COCRDUPC-018) and a failed read (500 with the file-error text, COCRDUPC-019) all
//! go to the domain, which builds the view and its snapshot (COCRDUPC-017, COCRDUPC-044). The
//! program words a failed read itself, so it never becomes `ServiceError::Store`.
//!
//! # Endpoint 10
//!
//! [`save`] hands the key, the raw detail fields and the echoed snapshot to `2000-DECIDE-ACTION`.
//! ENTER, F5 outside state `N` and every answer without a validated change read and write
//! nothing (COCRDUPC-007, COCRDUPC-020 to COCRDUPC-035, COCRDUPC-041). F5 with validated
//! changes runs `9200-WRITE-PROCESSING` in one transaction (`COCRDUPC.cbl:1420-1496`):
//!
//! 1. The card that the path's `card_ref` names is read, then read again by its number with its
//!    row locked: the `READ CARDDAT UPDATE` of lines 1427-1449. An unparseable or unknown handle,
//!    a failed read and a failed lock are all `Could not lock record for update`, state `L`
//!    (COCRDUPC-036, COCRDUPC-043).
//! 2. `9300-CHECK-CHANGE-IN-REC` compares the held record with the snapshot: a difference is the
//!    409 `Record changed by some one else. Please review`, state `S`, after which the screen
//!    takes the card's current values from endpoint 9 (`COCRDUPC.cbl:1498-1523`; COCRDUPC-037).
//! 3. `REWRITE` of the record the domain builds over the held one: number, CVV and `FILLER` from
//!    the held record, never from the request (`COCRDUPC.cbl:1461-1492`; COCRDUPC-038). Success
//!    commits and answers state `C` (COCRDUPC-039); no row rewritten, a refused value or a failed
//!    statement is `Update of record failed`, state `F`, nothing written (COCRDUPC-040).
//!
//! Every failure rolls the transaction back before it answers, which releases the row lock. The
//! lock is held only across these statements and the pure domain calls between them.
//!
//! # Redaction
//!
//! The card number stays inside its `CardNumber` wrapper and the CVV inside the held
//! `CardRecord`; neither reaches a response, and no request carries either after the search
//! (COCRDUPC-042, COCRDUPC-043). Nothing here formats, logs or traces a card number, a CVV, a
//! name or a record: a failed statement is logged by its `PersistenceError` alone, which holds an
//! SQLSTATE, a constraint name or a column name, never a value (Cardholder Data Redaction).

use domain::online::cocrdlic::CardRef;
use domain::online::cocrdupc::{
    self as rules, CardKey, CardLookup, CardSaveEffects, CardSnapshot, CardUpdateFailure,
    CardUpdateOutcome, CardUpdateView, Decision,
};
use persistence::{CardRepository, PersistenceError, PgStore};
use tracing::callsite::{Callsite as _, DefaultCallsite};
use tracing::field::{Value, display};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cocrdslc::CardLookupQuery;
use crate::dto::cocrdupc::CardUpdateRequest;
use crate::error::ServiceError;

/// Endpoint 9, ENTER with search keys or a list row's handle: the card as `3200-SETUP-SCREEN-VARS`
/// shows it in state `S`, with the snapshot of `CCUP-OLD-DETAILS` (COCRDUPC-002, COCRDUPC-016,
/// COCRDUPC-017, COCRDUPC-042, COCRDUPC-044).
///
/// The view carries the card's handle and its number only as `CardNumber`, whose serialisation is
/// redacted; it holds no CVV.
///
/// # Errors
///
/// [`ServiceError::Rule`] with the program's [`CardUpdateFailure`]: a key edit's 400 with no read
/// (COCRDUPC-010 to COCRDUPC-014), no card with the number or handle (404, COCRDUPC-003,
/// COCRDUPC-018), or a failed read (500, COCRDUPC-019). No other variant is returned.
pub(crate) async fn fetch(
    state: &AppState,
    _user: &SignedOnUser,
    query: CardLookupQuery,
) -> Result<CardUpdateView, ServiceError<CardUpdateFailure>> {
    // A clone copies only the pool handle; repository methods take `&mut self`.
    let mut store = state.store.clone();
    read_card(&mut store, query).await
}

/// [`fetch`] over any [`CardRepository`], so that the read and its mapping are exercised against
/// an in-memory table as well as `PostgreSQL`.
async fn read_card<R: CardRepository>(
    cards: &mut R,
    query: CardLookupQuery,
) -> Result<CardUpdateView, ServiceError<CardUpdateFailure>> {
    let CardLookupQuery {
        acct_id,
        card_num,
        card_ref,
    } = query;
    let (account, key) =
        rules::edit_keys(&acct_id, card_num.as_deref(), card_ref.map(CardRef::new))
            .map_err(ServiceError::Rule)?;

    // COCRDUPC-016: the read takes the card key alone; COCRDUPC-002: a handle stands for the
    // number COCRDLIC would have passed.
    let read = match &key {
        CardKey::ByNumber(number) => CardRepository::find_by_id(cards, number).await,
        CardKey::ByRef(handle) => CardRepository::find_by_ref(cards, *handle).await,
    };
    let lookup = match &read {
        // The account of the request, never the card's own (COCRDUPC-016).
        Ok(Some((handle, record))) => CardLookup::Found {
            acct_id: account,
            card_ref: *handle,
            record,
        },
        Ok(None) => CardLookup::NotFound,
        Err(error) => {
            // Written out: `tracing`'s `error!` expands to an `allow(unused_imports)` lint
            // suppression.
            static EVENT: DefaultCallsite = DefaultCallsite::new(&EVENT_METADATA);
            static EVENT_METADATA: Metadata<'static> = tracing::metadata! {
                name: concat!("event ", file!(), ":", line!()),
                target: module_path!(),
                level: Level::ERROR,
                fields: STORE_FAILED_FIELDS,
                callsite: &EVENT,
                kind: Kind::EVENT,
            };
            store_failed(
                &EVENT,
                "CARDDAT read failed; answering the COCRDUPC file error",
                error,
            );
            CardLookup::Error
        }
    };
    rules::fetch(&key, lookup).map_err(ServiceError::Rule)
}

/// Endpoint 10, ENTER or F5 on the card shown: the change check and the edits, then, for F5 in
/// state `N` with changes that pass, the save of `9200-WRITE-PROCESSING` on the card that
/// `card_ref` names (COCRDUPC-007, COCRDUPC-020 to COCRDUPC-041, COCRDUPC-043).
///
/// `card_ref` is the raw path text. It is read only by a save, as the COBOL reads the card only
/// there; the echoed snapshot is never a key and grants nothing (COCRDUPC-044).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the domain's [`CardUpdateFailure`]: the no-change 400 or an
///   edit's 400 of the decision (COCRDUPC-020 to COCRDUPC-032); the lock failure (COCRDUPC-036),
///   the 409 (COCRDUPC-037) and the rewrite failure (COCRDUPC-040) of the save.
/// - [`ServiceError::Store`] when the transaction cannot begin, commit or roll back: a store
///   failure with no audited text.
pub(crate) async fn save(
    state: &AppState,
    _user: &SignedOnUser,
    card_ref: &str,
    request: CardUpdateRequest,
) -> Result<CardUpdateOutcome, ServiceError<CardUpdateFailure>> {
    match decision(request).map_err(ServiceError::Rule)? {
        Decision::Respond(outcome) => Ok(outcome),
        Decision::Save(effects) => write_processing(&state.store, card_ref, &effects).await,
    }
}

/// `1200-EDIT-MAP-INPUTS` and `2000-DECIDE-ACTION` over the request as received: the key, the raw
/// text of the four detail fields and the echoed snapshot, whose state stands for
/// `CCUP-CHANGE-ACTION` (COCRDUPC-007, COCRDUPC-009, COCRDUPC-041, COCRDUPC-044).
///
/// # Errors
///
/// The domain's no-change or edit failure, each a 400 (COCRDUPC-020 to COCRDUPC-032).
fn decision(request: CardUpdateRequest) -> Result<Decision, CardUpdateFailure> {
    let CardUpdateRequest {
        aid,
        fields,
        snapshot,
    } = request;
    let old = CardSnapshot::from(&snapshot);
    rules::decide(aid.into(), &old, &fields.input())
}

/// `9200-WRITE-PROCESSING` for the changes F5 confirmed, in one transaction (COCRDUPC-036 to
/// COCRDUPC-040, COCRDUPC-043): [`rewrite`] runs its reads and its rewrite inside it, then the
/// transaction commits or, on any failure, rolls back.
///
/// # Errors
///
/// [`ServiceError::Rule`] with the failure [`rewrite`] returns, once the transaction is rolled
/// back, or with [`CardUpdateFailure::lock_failed`] before any transaction when `card_ref` is no
/// handle; [`ServiceError::Store`] when the transaction cannot begin, roll back or commit.
async fn write_processing(
    store: &PgStore,
    card_ref: &str,
    effects: &CardSaveEffects,
) -> Result<CardUpdateOutcome, ServiceError<CardUpdateFailure>> {
    let Some(handle) = parse_handle(card_ref) else {
        return Err(ServiceError::Rule(CardUpdateFailure::lock_failed()));
    };
    let mut tx = store.begin().await?;
    match rewrite(&mut tx, handle, effects).await {
        Ok(()) => {
            tx.commit().await?;
            Ok(effects.committed())
        }
        Err(failure) => {
            tx.rollback().await?;
            Err(ServiceError::Rule(failure))
        }
    }
}

/// The handle the path text names: a decimal `cards.card_ref` value. Total: any other text, an
/// empty or out-of-range one included, names no card (COCRDUPC-036, COCRDUPC-043).
fn parse_handle(card_ref: &str) -> Option<CardRef> {
    card_ref.parse::<i64>().ok().map(CardRef::new)
}

/// The statements of `9200-WRITE-PROCESSING` on `cards`, the caller's open transaction: resolve
/// `handle` to the card's number, `READ CARDDAT UPDATE` by that number, `9300-CHECK-CHANGE-IN-REC`
/// on the held record, then the `REWRITE` (`COCRDUPC.cbl:1420-1496`).
///
/// It commits nothing and rolls nothing back: an `Err` tells the caller to roll back, which also
/// releases the row lock, and `Ok` that the rewrite succeeded and the transaction may commit.
///
/// # Errors
///
/// - [`CardUpdateFailure::lock_failed`] when no card has `handle`, either read fails or the lock
///   finds the card gone (COCRDUPC-036);
/// - the 409 of `9300-CHECK-CHANGE-IN-REC` when the held record differs from the snapshot; nothing
///   is rewritten (COCRDUPC-037);
/// - [`CardUpdateFailure::rewrite_failed`] when the domain cannot build the rewritten record, the
///   rewrite changes no row or its statement fails (COCRDUPC-038, COCRDUPC-040).
async fn rewrite<R: CardRepository>(
    cards: &mut R,
    handle: CardRef,
    effects: &CardSaveEffects,
) -> Result<(), CardUpdateFailure> {
    // COCRDUPC-043: the key read and rewritten is the number of the card the handle names.
    let card = match CardRepository::find_by_ref(cards, handle).await {
        Ok(Some((_, record))) => record.num,
        Ok(None) => return Err(CardUpdateFailure::lock_failed()),
        Err(error) => {
            static EVENT: DefaultCallsite = DefaultCallsite::new(&EVENT_METADATA);
            static EVENT_METADATA: Metadata<'static> = tracing::metadata! {
                name: concat!("event ", file!(), ":", line!()),
                target: module_path!(),
                level: Level::ERROR,
                fields: STORE_FAILED_FIELDS,
                callsite: &EVENT,
                kind: Kind::EVENT,
            };
            store_failed(
                &EVENT,
                "CARDDAT handle read failed; answering the lock error",
                &error,
            );
            return Err(CardUpdateFailure::lock_failed());
        }
    };
    let held = match CardRepository::lock_by_id(cards, &card).await {
        Ok(Some((_, record))) => record,
        Ok(None) => return Err(CardUpdateFailure::lock_failed()),
        Err(error) => {
            static EVENT: DefaultCallsite = DefaultCallsite::new(&EVENT_METADATA);
            static EVENT_METADATA: Metadata<'static> = tracing::metadata! {
                name: concat!("event ", file!(), ":", line!()),
                target: module_path!(),
                level: Level::ERROR,
                fields: STORE_FAILED_FIELDS,
                callsite: &EVENT,
                kind: Kind::EVENT,
            };
            store_failed(
                &EVENT,
                "CARDDAT read for update failed; answering the lock error",
                &error,
            );
            return Err(CardUpdateFailure::lock_failed());
        }
    };
    effects.check_change_in_rec(&held)?;
    let record = effects.rewritten(&held)?;
    match CardRepository::update(cards, &record).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(CardUpdateFailure::rewrite_failed()),
        Err(error) => {
            static EVENT: DefaultCallsite = DefaultCallsite::new(&EVENT_METADATA);
            static EVENT_METADATA: Metadata<'static> = tracing::metadata! {
                name: concat!("event ", file!(), ":", line!()),
                target: module_path!(),
                level: Level::ERROR,
                fields: STORE_FAILED_FIELDS,
                callsite: &EVENT,
                kind: Kind::EVENT,
            };
            store_failed(
                &EVENT,
                "CARDDAT rewrite failed; answering the update error",
                &error,
            );
            Err(CardUpdateFailure::rewrite_failed())
        }
    }
}

/// The fields of the `ERROR` event of a failed `CARDDAT` statement, in the order
/// [`store_failed`] gives their values.
const STORE_FAILED_FIELDS: &[&str] = &["message", "error"];

/// Emits the event of `callsite`, a failed `CARDDAT` statement's, in the current span when
/// `runtime::telemetry::event_enabled` says so: `message` and the error's `Display`, which holds
/// an SQLSTATE, a constraint name or a column name and never a value.
fn store_failed(
    callsite: &'static DefaultCallsite,
    message: &'static str,
    error: &PersistenceError,
) {
    if runtime::telemetry::event_enabled(callsite) {
        let metadata = callsite.metadata();
        Event::dispatch(
            metadata,
            &metadata.fields().value_set_all(&[
                Some(&message as &dyn Value),
                Some(&display(error) as &dyn Value),
            ]),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use domain::copybook::{PicDate, PicU, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdupc::{
        CONFIRM_UPDATE_SUCCESS, COULD_NOT_LOCK_FOR_UPDATE, CardSaveEffects, CardUpdateFailure,
        CardUpdateOutcome, CardUpdateState, CardUpdateView, DATA_WAS_CHANGED_BEFORE_UPDATE,
        DID_NOT_FIND_ACCTCARD_COMBO, Decision, FILE_ERROR, FOUND_CARDS_FOR_ACCOUNT, FailureClass,
        LOCKED_BUT_UPDATE_FAILED, NO_SEARCH_CRITERIA_RECEIVED, PROMPT_FOR_CONFIRMATION,
    };
    use domain::records::card::CardRecord;
    use domain::sensitive::{CardNumber, Cvv};
    use persistence::repos::CardEntry;
    use persistence::{CardRepository, PersistenceError};

    use super::{decision, parse_handle, read_card, rewrite};
    use crate::dto::cocrdslc::CardLookupQuery;
    use crate::dto::cocrdupc::{Aid, CardUpdateFields, CardUpdateRequest};
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1111222233334444";

    /// [`CARD`] as twelve `*` and its last four digits.
    const REDACTED: &str = "************4444";

    /// The account the requests name; the card belongs to account 99.
    const ACCT: &str = "00000000050";

    /// The `card_ref` of [`CARD`].
    const HANDLE: i64 = 7;

    /// The stored CVV, which no request carries and the save keeps.
    const CVV: u64 = 321;

    /// The repository calls a double received.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Calls {
        /// `find_by_id`, the read by typed number.
        by_id: usize,
        /// `find_by_ref`, the read by handle.
        by_ref: usize,
        /// `lock_by_id`, the `READ … UPDATE`.
        lock: usize,
        /// `update`, the `REWRITE`.
        update: usize,
        /// Any other method, none of which endpoints 9 and 10 issue.
        other: usize,
    }

    /// The statement a double fails, as the database would.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fault {
        /// Every statement succeeds.
        None,
        /// `find_by_id` and `find_by_ref` fail.
        Read,
        /// `lock_by_id` fails, as a lock timeout does.
        Lock,
        /// `update` fails.
        Update,
        /// `update` changes no row.
        UpdateMisses,
    }

    /// An in-memory `cards` table. Methods off the path of endpoints 9 and 10 are counted and
    /// refused, so a stray call shows in [`Calls::other`].
    struct Cards {
        table: Vec<CardEntry>,
        fault: Fault,
        calls: Calls,
    }

    impl Cards {
        fn holding(table: Vec<CardEntry>) -> Self {
            Self {
                table,
                fault: Fault::None,
                calls: Calls::default(),
            }
        }

        fn find(
            &self,
            fails: bool,
            hit: impl Fn(&CardEntry) -> bool,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            if fails {
                return Err(PersistenceError::Unavailable);
            }
            Ok(self.table.iter().find(|entry| hit(entry)).cloned())
        }

        /// The stored record of [`CARD`].
        fn stored(&self) -> Result<&CardRecord, Box<dyn StdError>> {
            let card = CardNumber::parse(CARD)?;
            self.table
                .iter()
                .find(|(_, record)| record.num == card)
                .map(|(_, record)| record)
                .ok_or_else(|| "the card is gone".into())
        }

        fn refuse<T>(&mut self) -> Result<T, PersistenceError> {
            self.calls.other += 1;
            Err(PersistenceError::Unavailable)
        }
    }

    impl CardRepository for Cards {
        async fn find_by_id(
            &mut self,
            card: &CardNumber,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            self.calls.by_id += 1;
            self.find(self.fault == Fault::Read, |(_, record)| &record.num == card)
        }

        async fn lock_by_id(
            &mut self,
            card: &CardNumber,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            self.calls.lock += 1;
            self.find(self.fault == Fault::Lock, |(_, record)| &record.num == card)
        }

        async fn find_by_ref(
            &mut self,
            handle: CardRef,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            self.calls.by_ref += 1;
            self.find(self.fault == Fault::Read, |(card_ref, _)| {
                *card_ref == handle
            })
        }

        async fn page_after(
            &mut self,
            _start: Option<&CardNumber>,
            _acct: Option<PicU<11>>,
            _card: Option<&CardNumber>,
            _limit: i64,
        ) -> Result<Vec<CardEntry>, PersistenceError> {
            self.refuse()
        }

        async fn page_before(
            &mut self,
            _start: Option<&CardNumber>,
            _acct: Option<PicU<11>>,
            _card: Option<&CardNumber>,
            _limit: i64,
        ) -> Result<Vec<CardEntry>, PersistenceError> {
            self.refuse()
        }

        async fn find_all(&mut self) -> Result<Vec<CardEntry>, PersistenceError> {
            self.refuse()
        }

        async fn insert(&mut self, _record: &CardRecord) -> Result<(), PersistenceError> {
            self.refuse()
        }

        async fn update(&mut self, record: &CardRecord) -> Result<bool, PersistenceError> {
            self.calls.update += 1;
            match self.fault {
                Fault::Update => Err(PersistenceError::Unavailable),
                Fault::UpdateMisses => Ok(false),
                Fault::None | Fault::Read | Fault::Lock => {
                    let row = self
                        .table
                        .iter_mut()
                        .find(|(_, stored)| stored.num == record.num);
                    Ok(row.map(|(_, stored)| *stored = record.clone()).is_some())
                }
            }
        }

        async fn replace_all(&mut self, _records: &[CardRecord]) -> Result<u64, PersistenceError> {
            self.refuse()
        }
    }

    fn date(year: i32, month: u32, day: u32) -> Result<PicDate, Box<dyn StdError>> {
        let value = NaiveDate::from_ymd_opt(year, month, day).ok_or("not a calendar date")?;
        Ok(PicDate::new(Some(value))?)
    }

    /// The table holding [`CARD`] under [`HANDLE`]: an active card of account 99, not of
    /// [`ACCT`], named `Test Holder` and expiring 2025-04-30.
    fn table() -> Result<Cards, Box<dyn StdError>> {
        let record = CardRecord {
            num: CardNumber::parse(CARD)?,
            acct_id: PicU::new(99)?,
            cvv_cd: Cvv::new(PicU::new(CVV)?),
            embossed_name: PicX::from_text("Test Holder")?,
            expiraion_date: date(2025, 4, 30)?,
            active_status: PicX::from_text("Y")?,
            filler: PicX::spaces(),
        };
        Ok(Cards::holding(vec![(CardRef::new(HANDLE), record)]))
    }

    /// The query string of one endpoint 9 request.
    fn query(acct: &str, card_num: Option<&str>, card_ref: Option<i64>) -> CardLookupQuery {
        CardLookupQuery {
            acct_id: acct.to_owned(),
            card_num: card_num.map(str::to_owned),
            card_ref,
        }
    }

    /// Changed values that pass every edit.
    fn valid_change() -> CardUpdateFields {
        CardUpdateFields {
            embossed_name: "New Holder".to_owned(),
            status: "N".to_owned(),
            expiry_month: "06".to_owned(),
            expiry_year: "2030".to_owned(),
        }
    }

    /// The program failure `outcome` holds; any other outcome fails the test.
    fn rule<T>(
        outcome: Result<T, ServiceError<CardUpdateFailure>>,
    ) -> Result<CardUpdateFailure, Box<dyn StdError>> {
        match outcome {
            Err(ServiceError::Rule(failure)) => Ok(failure),
            Err(other) => Err(format!("not a program failure: {other}").into()),
            Ok(_) => Err("the request succeeded".into()),
        }
    }

    /// Endpoint 9 by handle, then the endpoint 10 request that `aid` sends over [`valid_change`]
    /// with the fetched snapshot in `state`.
    async fn request_after_fetch(
        cards: &mut Cards,
        aid: Aid,
        state: CardUpdateState,
    ) -> Result<CardUpdateRequest, Box<dyn StdError>> {
        let view = read_card(cards, query(ACCT, None, Some(HANDLE))).await?;
        Ok(CardUpdateRequest {
            aid,
            fields: valid_change(),
            snapshot: view.snapshot.with_state(state).into(),
        })
    }

    /// The save F5 confirms for [`valid_change`] after endpoint 9 fetched the card by handle; the
    /// double's call counts are reset afterwards.
    async fn confirmed_save(cards: &mut Cards) -> Result<CardSaveEffects, Box<dyn StdError>> {
        let request =
            request_after_fetch(cards, Aid::Pf5, CardUpdateState::ChangesOkNotConfirmed).await?;
        cards.calls = Calls::default();
        match decision(request)? {
            Decision::Save(effects) => Ok(effects),
            Decision::Respond(_) => Err("F5 in state N with valid changes did not save".into()),
        }
    }

    // audit: COCRDUPC-002
    // audit: COCRDUPC-016
    // audit: COCRDUPC-017
    // audit: COCRDUPC-042
    // audit: COCRDUPC-044
    #[test]
    fn handle_and_number_read_the_card_alone_shown_redacted_under_the_account_entered() -> TestResult
    {
        block_on(async {
            let mut cards = table()?;

            let by_handle = read_card(&mut cards, query(ACCT, None, Some(HANDLE))).await?;
            let by_number = read_card(&mut cards, query(ACCT, Some(CARD), None)).await?;

            assert_eq!(
                cards.calls,
                Calls {
                    by_id: 1,
                    by_ref: 1,
                    ..Calls::default()
                }
            );
            for view in [&by_handle, &by_number] {
                let CardUpdateView {
                    card_ref,
                    card_num,
                    acct_id,
                    embossed_name,
                    expiration_date,
                    status,
                    snapshot,
                    info_message,
                } = view;
                assert_eq!(*card_ref, CardRef::new(HANDLE));
                assert_eq!(card_num.redacted(), REDACTED);
                assert_eq!(*acct_id, PicU::new(50)?);
                assert_eq!(*embossed_name, PicX::from_text("TEST HOLDER")?);
                assert_eq!(*expiration_date, date(2025, 4, 30)?.value());
                assert_eq!(*status, PicX::from_text("Y")?);
                assert_eq!(snapshot.state(), CardUpdateState::ShowDetails);
                assert_eq!(snapshot.acct_id(), Some(PicU::new(50)?));
                assert_eq!(*info_message, FOUND_CARDS_FOR_ACCOUNT);
            }
            Ok(())
        })
    }

    // audit: COCRDUPC-003
    // audit: COCRDUPC-018
    #[test]
    fn an_unknown_handle_or_number_is_not_found() -> TestResult {
        block_on(async {
            let mut cards = table()?;

            for (request, state) in [
                (
                    query(ACCT, None, Some(HANDLE + 1)),
                    CardUpdateState::ShowDetails,
                ),
                (
                    query(ACCT, Some("9999888877776666"), None),
                    CardUpdateState::NotFetched,
                ),
            ] {
                let failure = rule(read_card(&mut cards, request).await)?;
                assert_eq!(failure.message(), DID_NOT_FIND_ACCTCARD_COMBO);
                assert_eq!(failure.class(), FailureClass::NotFound);
                assert_eq!(failure.state(), state);
            }
            Ok(())
        })
    }

    // audit: COCRDUPC-019
    #[test]
    fn a_failed_read_is_the_file_error_not_a_store_error() -> TestResult {
        block_on(async {
            let mut cards = table()?;
            cards.fault = Fault::Read;

            for request in [
                query(ACCT, None, Some(HANDLE)),
                query(ACCT, Some(CARD), None),
            ] {
                let failure = rule(read_card(&mut cards, request).await)?;
                assert_eq!(failure.message(), FILE_ERROR);
                assert_eq!(failure.class(), FailureClass::Store);
            }
            assert_eq!(cards.calls.other, 0);
            Ok(())
        })
    }

    // audit: COCRDUPC-010
    #[test]
    fn a_key_edit_failure_reads_no_card() -> TestResult {
        block_on(async {
            let mut cards = table()?;

            let failure = rule(read_card(&mut cards, query("", None, None)).await)?;

            assert_eq!(failure.message(), NO_SEARCH_CRITERIA_RECEIVED);
            assert_eq!(failure.class(), FailureClass::Input);
            assert_eq!(cards.calls, Calls::default());
            Ok(())
        })
    }

    // audit: COCRDUPC-007
    // audit: COCRDUPC-034
    #[test]
    fn enter_and_f5_before_confirmation_validate_without_saving() -> TestResult {
        block_on(async {
            let mut cards = table()?;

            for (aid, state) in [
                (Aid::Enter, CardUpdateState::ShowDetails),
                (Aid::Pf5, CardUpdateState::ShowDetails),
                (Aid::Pf5, CardUpdateState::ChangesNotOk),
            ] {
                let request = request_after_fetch(&mut cards, aid, state).await?;
                let Decision::Respond(outcome) = decision(request)? else {
                    return Err(format!("{aid:?} in state {state:?} asked for a save").into());
                };
                assert_eq!(outcome.state(), CardUpdateState::ChangesOkNotConfirmed);
                assert_eq!(outcome.info_message(), PROMPT_FOR_CONFIRMATION);
                assert!(outcome.confirmation_keys_shown());
            }
            assert_eq!(cards.calls.lock + cards.calls.update + cards.calls.other, 0);
            Ok(())
        })
    }

    // audit: COCRDUPC-038
    // audit: COCRDUPC-039
    // audit: COCRDUPC-043
    #[test]
    fn a_confirmed_save_rewrites_the_held_card_keeping_its_number_and_cvv() -> TestResult {
        block_on(async {
            let mut cards = table()?;
            let effects = confirmed_save(&mut cards).await?;

            rewrite(&mut cards, CardRef::new(HANDLE), &effects).await?;

            assert_eq!(
                cards.calls,
                Calls {
                    by_ref: 1,
                    lock: 1,
                    update: 1,
                    ..Calls::default()
                }
            );
            let stored = cards.stored()?;
            assert_eq!(stored.num, CardNumber::parse(CARD)?);
            assert_eq!(stored.cvv_cd, Cvv::new(PicU::new(CVV)?));
            assert_eq!(stored.acct_id, PicU::new(50)?);
            assert_eq!(stored.embossed_name, PicX::from_text("New Holder")?);
            assert_eq!(stored.expiraion_date, date(2030, 6, 30)?);
            assert_eq!(stored.active_status, PicX::from_text("N")?);
            assert_eq!(stored.filler, PicX::spaces());

            let committed = effects.committed();
            assert!(matches!(committed, CardUpdateOutcome::Committed { .. }));
            assert_eq!(committed.state(), CardUpdateState::ChangesOkayedAndDone);
            assert_eq!(committed.info_message(), CONFIRM_UPDATE_SUCCESS);
            Ok(())
        })
    }

    // audit: COCRDUPC-036
    #[test]
    fn a_card_that_cannot_be_read_for_update_is_not_locked_and_not_written() -> TestResult {
        block_on(async {
            for (fault, handle, locks) in [
                (Fault::None, HANDLE + 1, 0),
                (Fault::Read, HANDLE, 0),
                (Fault::Lock, HANDLE, 1),
            ] {
                let mut cards = table()?;
                let effects = confirmed_save(&mut cards).await?;
                let before = cards.stored()?.clone();
                cards.fault = fault;

                let failure = rewrite(&mut cards, CardRef::new(handle), &effects)
                    .await
                    .err()
                    .ok_or("the save succeeded")?;

                assert_eq!(failure.message(), COULD_NOT_LOCK_FOR_UPDATE);
                assert_eq!(failure.class(), FailureClass::Store);
                assert_eq!(failure.state(), CardUpdateState::LockError);
                assert_eq!(cards.calls.lock, locks);
                assert_eq!(cards.calls.update, 0);
                assert_eq!(*cards.stored()?, before);
            }
            Ok(())
        })
    }

    // audit: COCRDUPC-037
    #[test]
    fn a_card_changed_since_the_fetch_is_a_conflict_and_is_not_rewritten() -> TestResult {
        block_on(async {
            let mut cards = table()?;
            let effects = confirmed_save(&mut cards).await?;
            // Another user deactivates the card between the fetch and the save.
            let changed = CardRecord {
                active_status: PicX::from_text("N")?,
                ..cards.stored()?.clone()
            };
            cards.table = vec![(CardRef::new(HANDLE), changed.clone())];

            let failure = rewrite(&mut cards, CardRef::new(HANDLE), &effects)
                .await
                .err()
                .ok_or("the save succeeded")?;

            assert_eq!(failure.message(), DATA_WAS_CHANGED_BEFORE_UPDATE);
            assert_eq!(failure.class(), FailureClass::Conflict);
            assert_eq!(failure.state(), CardUpdateState::ShowDetails);
            assert_eq!(cards.calls.lock, 1);
            assert_eq!(cards.calls.update, 0);
            assert_eq!(*cards.stored()?, changed);
            Ok(())
        })
    }

    // audit: COCRDUPC-040
    #[test]
    fn a_failed_rewrite_is_the_update_failure() -> TestResult {
        block_on(async {
            for fault in [Fault::Update, Fault::UpdateMisses] {
                let mut cards = table()?;
                let effects = confirmed_save(&mut cards).await?;
                cards.fault = fault;

                let failure = rewrite(&mut cards, CardRef::new(HANDLE), &effects)
                    .await
                    .err()
                    .ok_or("the save succeeded")?;

                assert_eq!(failure.message(), LOCKED_BUT_UPDATE_FAILED);
                assert_eq!(failure.class(), FailureClass::Store);
                assert_eq!(failure.state(), CardUpdateState::UpdateFailed);
                assert_eq!(cards.calls.update, 1);
            }
            Ok(())
        })
    }

    // audit: COCRDUPC-043
    #[test]
    fn the_path_handle_is_parsed_totally() {
        assert_eq!(parse_handle("7"), Some(CardRef::new(HANDLE)));
        for text in ["", "abc", "7 ", "0x7", "99999999999999999999", "\u{0}"] {
            assert_eq!(parse_handle(text), None, "{text:?}");
        }
        assert_eq!(
            CardUpdateFailure::lock_failed().message(),
            COULD_NOT_LOCK_FOR_UPDATE
        );
    }
}
