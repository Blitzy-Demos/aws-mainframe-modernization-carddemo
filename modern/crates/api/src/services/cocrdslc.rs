//! COCRDSLC, the card detail: the service of endpoint 8, `GET /api/v1/cards/view` (transaction
//! `CCDL`; AAP 0.3.4.1), called only by `api::handlers::cocrdslc::view`.
//!
//! It ports `9000-READ-DATA`, which performs only `9100-GETCARD-BYACCTCARD`: a `READ` of
//! `CARDDAT` by the 16-byte card number alone (`COCRDSLC.cbl:726-775`, COCRDSLC-014). The request
//! may instead name the card by the `cards.card_ref` handle of a COCRDLI list row, which is read
//! in place of the number COCRDLIC passes on its `S` selection (COCRDSLC-006).
//!
//! # Flow
//!
//! 1. `domain::online::cocrdslc::edit` runs the program's edits over the raw query text and
//!    yields the account and a `CardKey`, or the audited 400; no read takes place then
//!    (COCRDSLC-007 to COCRDSLC-013).
//! 2. The card is read by that key alone: `CardRepository::find_by_id` for a typed number,
//!    `CardRepository::find_by_ref` for a handle. The account entered plays no part in the read
//!    and is never compared with the card's own `CARD-ACCT-ID` (COCRDSLC-014).
//! 3. The store's answer becomes a `CardLookup`: a row is `Found`, no row is `NotFound` (404,
//!    COCRDSLC-015), and any store error is `Error` (500 with the file-error text of COCRDSLC-016,
//!    whose `RESP` fields are `HALTED(H4)`, COCRDSLC-017). The program answers a failed read with
//!    a message of its own, so a store error here is a domain outcome and never becomes
//!    `ServiceError::Store`.
//! 4. `domain::online::cocrdslc::view` builds the response for a found card or the failure
//!    (COCRDSLC-015 to COCRDSLC-023).
//!
//! # Disclosure (Cardholder Data Redaction, AAP 0.3.4.8, 0.5.1)
//!
//! Endpoint 8 is the one operation that may return all 16 digits of a card number. The only input
//! to that choice is the `usr_type` of the verified token, `SignedOnUser::user_type`, which this
//! service hands to the domain: the full number for `A` (COCRDSLC-022), twelve `*` followed by the
//! last four digits for every other type (COCRDSLC-023). No query field, header or other request
//! value reaches it. The rule's owner clause never grants disclosure: `CSUSR01Y` holds a user's
//! ID, names, password and type only (`CSUSR01Y.cpy:17-23`), so no user is linked to a card and
//! ownership is never checked.
//!
//! Nothing here formats, logs or traces a card number, typed or stored, or the view that holds
//! one. A failed read is logged by its `PersistenceError` alone, which carries an SQLSTATE, a
//! constraint name or a column name, never a value.

use domain::online::cocrdlic::CardRef;
use domain::online::cocrdslc::{self, CardKey, CardLookup, CardView, DetailFailure};
use persistence::CardRepository;
use tracing::callsite::DefaultCallsite;
use tracing::field::{Value, display};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cocrdslc::CardLookupQuery;
use crate::error::ServiceError;

/// Endpoint 8, ENTER: the card the query names, as `user` may see it (COCRDSLC-006 to
/// COCRDSLC-023).
///
/// The edits run over the raw `acct_id`, `card_num` and `card_ref` of `query`; the card is read
/// through the state's store by the typed number or by the handle; the number in the returned
/// [`CardView`] is full only when `user.user_type` is `A` (COCRDSLC-022, COCRDSLC-023).
///
/// # Errors
///
/// [`ServiceError::Rule`] with the program's [`DetailFailure`]: an edit failure (400,
/// COCRDSLC-007 to COCRDSLC-013), no card with the number or handle (404, COCRDSLC-015), or a
/// failed read (500, COCRDSLC-016). No other variant is returned.
pub(crate) async fn view(
    state: &AppState,
    user: &SignedOnUser,
    query: CardLookupQuery,
) -> Result<CardView, ServiceError<DetailFailure>> {
    // A clone copies only the pool handle; repository methods take `&mut self`.
    let mut store = state.store.clone();
    detail(&mut store, user, query).await
}

/// [`view`] over any [`CardRepository`], so that the read and its mapping are exercised against
/// an in-memory table as well as `PostgreSQL`.
async fn detail<R: CardRepository>(
    cards: &mut R,
    user: &SignedOnUser,
    query: CardLookupQuery,
) -> Result<CardView, ServiceError<DetailFailure>> {
    let CardLookupQuery {
        acct_id,
        card_num,
        card_ref,
    } = query;
    let (account, key) = cocrdslc::edit(&acct_id, card_num.as_deref(), card_ref.map(CardRef::new))
        .map_err(ServiceError::Rule)?;

    // COCRDSLC-014: the read takes the card key alone; COCRDSLC-006: a handle stands for the
    // number COCRDLIC would have passed.
    let read = match key {
        CardKey::ByNumber(number) => CardRepository::find_by_id(cards, &number).await,
        CardKey::ByRef(handle) => CardRepository::find_by_ref(cards, handle).await,
    };
    let lookup = match &read {
        // The account of the request, never the card's own (COCRDSLC-018).
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
                fields: &["message", "error"],
                callsite: &EVENT,
                kind: Kind::EVENT,
            };
            if runtime::telemetry::event_enabled(&EVENT) {
                Event::dispatch(
                    &EVENT_METADATA,
                    &EVENT_METADATA.fields().value_set_all(&[
                        Some(
                            &"CARDDAT read failed; answering the COCRDSLC file error" as &dyn Value,
                        ),
                        Some(&display(error) as &dyn Value),
                    ]),
                );
            }
            CardLookup::Error
        }
    };
    cocrdslc::view(&user.user_type, lookup).map_err(ServiceError::Rule)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use domain::copybook::{PicDate, PicU, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdslc::{
        CARD_FILTER_INVALID, CardNumberDisclosure, CardView, DID_NOT_FIND_ACCTCARD_COMBO,
        DetailFailure, FILE_ERROR, FailureClass, NO_SEARCH_CRITERIA_RECEIVED,
    };
    use domain::online::cosgn00c::UserType;
    use domain::records::card::CardRecord;
    use domain::sensitive::{CardNumber, Cvv};
    use persistence::repos::CardEntry;
    use persistence::{CardRepository, PersistenceError};
    use serde_json::json;

    use super::detail;
    use crate::auth::extract::SignedOnUser;
    use crate::dto::cocrdslc::CardLookupQuery;
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

    /// The repository calls a double received.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Calls {
        /// `find_by_id`, the read by typed number.
        by_id: usize,
        /// `find_by_ref`, the read by handle.
        by_ref: usize,
        /// Any other method, none of which endpoint 8 issues.
        other: usize,
    }

    /// An in-memory `cards` table, or one whose every read fails as an unreachable database
    /// does. Methods off endpoint 8's path are counted and refused, so a stray call shows both in
    /// [`Calls::other`] and as a failed read.
    struct Cards {
        table: Result<Vec<CardEntry>, PersistenceError>,
        calls: Calls,
    }

    impl Cards {
        fn holding(rows: Vec<CardEntry>) -> Self {
            Self {
                table: Ok(rows),
                calls: Calls::default(),
            }
        }

        fn unreachable() -> Self {
            Self {
                table: Err(PersistenceError::Unavailable),
                calls: Calls::default(),
            }
        }

        fn find(
            &self,
            hit: impl Fn(&CardEntry) -> bool,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            match &self.table {
                Ok(rows) => Ok(rows.iter().find(|entry| hit(entry)).cloned()),
                Err(error) => Err(error.clone()),
            }
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
            self.find(|(_, record)| &record.num == card)
        }

        async fn lock_by_id(
            &mut self,
            _card: &CardNumber,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            self.refuse()
        }

        async fn find_by_ref(
            &mut self,
            handle: CardRef,
        ) -> Result<Option<CardEntry>, PersistenceError> {
            self.calls.by_ref += 1;
            self.find(|(card_ref, _)| *card_ref == handle)
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

        async fn update(&mut self, _record: &CardRecord) -> Result<bool, PersistenceError> {
            self.refuse()
        }

        async fn replace_all(&mut self, _records: &[CardRecord]) -> Result<u64, PersistenceError> {
            self.refuse()
        }
    }

    /// A signed-on user of `user_type`, as the verified token yields it.
    fn user(user_type: UserType) -> Result<SignedOnUser, Box<dyn StdError>> {
        Ok(SignedOnUser {
            user_id: PicX::from_text("USER0001")?,
            user_type,
        })
    }

    /// The table holding [`CARD`] under [`HANDLE`]: a card of account 99, not of [`ACCT`].
    fn table() -> Result<Cards, Box<dyn StdError>> {
        let expiry = NaiveDate::from_ymd_opt(2025, 4, 30).ok_or("2025-04-30 is a calendar date")?;
        let record = CardRecord {
            num: CardNumber::parse(CARD)?,
            acct_id: PicU::new(99)?,
            cvv_cd: Cvv::new(PicU::new(321)?),
            embossed_name: PicX::from_text("Test Holder")?,
            expiraion_date: PicDate::new(Some(expiry))?,
            active_status: PicX::from_text("Y")?,
            filler: PicX::spaces(),
        };
        Ok(Cards::holding(vec![(CardRef::new(HANDLE), record)]))
    }

    /// The query string of one request.
    fn query(acct: &str, card_num: Option<&str>, card_ref: Option<i64>) -> CardLookupQuery {
        CardLookupQuery {
            acct_id: acct.to_owned(),
            card_num: card_num.map(str::to_owned),
            card_ref,
        }
    }

    /// The program failure `outcome` holds; any other outcome fails the test.
    fn rule(
        outcome: Result<CardView, ServiceError<DetailFailure>>,
    ) -> Result<DetailFailure, Box<dyn StdError>> {
        match outcome {
            Err(ServiceError::Rule(failure)) => Ok(failure),
            Err(other) => Err(format!("not a program failure: {other}").into()),
            Ok(_) => Err("a card was returned".into()),
        }
    }

    // audit: COCRDSLC-006
    // audit: COCRDSLC-014
    // audit: COCRDSLC-018
    // audit: COCRDSLC-023
    #[test]
    fn number_and_handle_read_the_same_card_shown_redacted_to_a_user() -> TestResult {
        block_on(async {
            let user = user(UserType::USER)?;
            let mut cards = table()?;

            let by_number = detail(&mut cards, &user, query(ACCT, Some(CARD), None)).await?;
            let by_handle = detail(&mut cards, &user, query(ACCT, None, Some(HANDLE))).await?;

            assert_eq!(
                cards.calls,
                Calls {
                    by_id: 1,
                    by_ref: 1,
                    other: 0
                }
            );
            assert_eq!(by_number, by_handle);
            assert_eq!(by_number.acct_id, PicU::new(50)?);
            assert_eq!(by_number.card_ref, CardRef::new(HANDLE));
            let CardNumberDisclosure::Redacted(number) = &by_number.card_num else {
                return Err("a user token was shown the full card number".into());
            };
            assert_eq!(number.redacted(), REDACTED);
            Ok(())
        })
    }

    // audit: COCRDSLC-022
    #[test]
    fn an_administrator_token_is_shown_the_full_number() -> TestResult {
        block_on(async {
            let mut cards = table()?;
            let view = detail(
                &mut cards,
                &user(UserType::ADMIN)?,
                query(ACCT, None, Some(HANDLE)),
            )
            .await?;

            let CardNumberDisclosure::Full(number) = &view.card_num else {
                return Err("an administrator token was shown the redacted number".into());
            };
            assert_eq!(serde_json::to_value(number)?, json!(CARD));
            Ok(())
        })
    }

    // audit: COCRDSLC-015
    #[test]
    fn an_unknown_number_or_handle_is_not_found() -> TestResult {
        block_on(async {
            let user = user(UserType::USER)?;
            let mut cards = table()?;

            for request in [
                query(ACCT, Some("9999888877776666"), None),
                query(ACCT, None, Some(HANDLE + 1)),
            ] {
                let failure = rule(detail(&mut cards, &user, request).await)?;
                assert_eq!(failure.message(), DID_NOT_FIND_ACCTCARD_COMBO);
                assert_eq!(failure.class(), FailureClass::NotFound);
            }
            Ok(())
        })
    }

    // audit: COCRDSLC-016
    #[test]
    fn a_failed_read_is_the_file_error_not_a_store_error() -> TestResult {
        block_on(async {
            let user = user(UserType::ADMIN)?;
            let mut cards = Cards::unreachable();

            for request in [
                query(ACCT, Some(CARD), None),
                query(ACCT, None, Some(HANDLE)),
            ] {
                let failure = rule(detail(&mut cards, &user, request).await)?;
                assert_eq!(failure.message(), FILE_ERROR);
                assert_eq!(failure.class(), FailureClass::Store);
            }
            assert_eq!(cards.calls.other, 0);
            Ok(())
        })
    }

    // audit: COCRDSLC-007
    // audit: COCRDSLC-008
    #[test]
    fn an_edit_failure_reads_no_card() -> TestResult {
        block_on(async {
            let user = user(UserType::ADMIN)?;
            let mut cards = table()?;

            for (request, message) in [
                (query("", None, None), NO_SEARCH_CRITERIA_RECEIVED),
                (query(ACCT, Some(CARD), Some(HANDLE)), CARD_FILTER_INVALID),
            ] {
                assert_eq!(
                    rule(detail(&mut cards, &user, request).await)?.message(),
                    message
                );
            }
            assert_eq!(cards.calls, Calls::default());
            Ok(())
        })
    }
}
