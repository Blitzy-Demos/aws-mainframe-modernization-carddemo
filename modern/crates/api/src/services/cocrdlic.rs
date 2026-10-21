//! COCRDLIC, the card list: the service of endpoint 7, `GET /api/v1/cards` (transaction `CCLI`,
//! map `CCRDLIA`, route `/cards`; AAP 0.3.4.1). Only `api::handlers::cocrdlic` calls it.
//!
//! It runs the browses of `9000-READ-FORWARD` (`STARTBR GTEQ`, then `READNEXT`,
//! `COCRDLIC.cbl:1123-1262`) and `9100-READ-BACKWARDS` (`STARTBR GTEQ`, then `READPREV`,
//! `COCRDLIC.cbl:1264-1380`) against the card repository, with the filters of
//! `9500-FILTER-RECORDS` (`COCRDLIC.cbl:1382-1411`) applied in the query. Every edit, paging
//! decision, key and message belongs to `domain::online::cocrdlic`; this module only loads what
//! the domain's [`ListPlan`] asks for and hands it back.
//!
//! # Flow
//!
//! 1. `edit_filters` over the raw filter text (COCRDLIC-005 to COCRDLIC-010); a failed edit is
//!    the 400 of the domain's `ListFailure` and reads nothing.
//! 2. The page size from `api::dto::resolve_page_size`, seven when absent (COCRDLIC-017;
//!    Bounded Pagination, `page_size` already validated to 1–100).
//! 3. `plan` from the key and the paging state the screen echoes, or `plan_reread` for the
//!    selection-error re-read (COCRDLIC-014, COCRDLIC-018, COCRDLIC-023 to COCRDLIC-027).
//! 4. The plan's start key, a `card_ref` handle, resolved to its card number with
//!    `CardRepository::find_by_ref`; then one filtered read of at most `ListPlan::limit` rows
//!    (`queries/cards_page_forward.sql` or `queries/cards_page_backward.sql`), rows passed to the
//!    domain exactly as returned.
//! 5. When the forward read fills the page, the look-ahead `READNEXT` of COCRDLIC-019: the card
//!    after the page's last row, read without the filters, matching or not.
//! 6. `page` decides the rows, keys, page number and both message lines (COCRDLIC-017 to
//!    COCRDLIC-034).
//!
//! No read takes more than `page_size + 1` rows, and every read is served by `cards_pkey`
//! (Bounded Pagination).
//!
//! # Identity
//!
//! The program never tests the user type, so every signed-on user lists every card, with no
//! account restriction (COCRDLIC-001): the signed-on user is taken only to prove the request
//! carries a verified token.
//!
//! # Store failures
//!
//! Every repository call here is a read of `CARDDAT`, and COBOL answers a failed read with the
//! file-error text (COCRDLIC-031), so each store error becomes `ListFailure::file_error`, a 500
//! whose `RESP` fields are `HALTED(H4)` (COCRDLIC-037). The error is logged with its SQLSTATE and
//! constraint name only.
//!
//! # Cardholder Data Redaction
//!
//! Keys travel as `card_ref` handles, never card numbers (`E32`, COCRDLIC-032). A card number
//! reaches the repository only as the bind of a browse key or of the card filter, and nothing
//! here formats one, a filter or a request.

use domain::online::cocrdlic::{self as rules, CardPage, CardRef, ListFailure, ListPlan, Read};
use domain::sensitive::CardNumber;
use persistence::{CardRepository, PersistenceError};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cocrdlic::CardListQuery;
use crate::dto::resolve_page_size;
use crate::error::ServiceError;

/// The rows of the look-ahead browse: the page's last card itself, which `STARTBR GTEQ` at its
/// key returns first, and the card after it, which the second `READNEXT` returns (COCRDLIC-019).
const LOOKAHEAD_ROWS: i64 = 2;

/// Endpoint 7: one page of the card list for `query`, the filters as typed, the key pressed and
/// the paging state of the previous response (COCRDLIC-001 to COCRDLIC-034).
///
/// # Errors
///
/// - [`ServiceError::Rule`] with the domain's filter-edit `ListFailure` (400) when a filter is
///   neither blank nor all digits (COCRDLIC-006, COCRDLIC-008, COCRDLIC-009; the evident path of
///   halt candidate `X-COCRDLIC-H2`), before any read.
/// - [`ServiceError::Rule`] with `ListFailure::file_error` (500) when a card read fails
///   (COCRDLIC-031).
/// - [`ServiceError::Internal`] only if the page size cannot be expressed as a row count, which
///   the validated range of 1 to 100 rules out.
pub(crate) async fn list(
    state: &AppState,
    _user: &SignedOnUser,
    query: CardListQuery,
) -> Result<CardPage, ServiceError<ListFailure>> {
    // Repository methods take `&mut self`; the clone copies only the pool handle.
    let mut store = state.store.clone();
    browse(&mut store, query).await
}

/// [`list`] over any card repository: the store of the running API, or a test double.
async fn browse<R: CardRepository>(
    store: &mut R,
    query: CardListQuery,
) -> Result<CardPage, ServiceError<ListFailure>> {
    let state = rules::ListState::from(&query);
    let CardListQuery {
        acct_id,
        card_num,
        direction,
        reread,
        page_size,
        ..
    } = query;
    let filters =
        rules::edit_filters(acct_id.as_deref(), card_num.as_deref()).map_err(ServiceError::Rule)?;
    let size = page_rows(page_size)?;
    let key = direction.map(rules::Direction::from);
    let plan = if reread == Some(true) {
        rules::plan_reread(key, state, size)
    } else {
        rules::plan(key, state, size)
    };
    let limit = row_limit(&plan)?;
    let entries = match plan.read() {
        Read::Forward { start } => {
            let from = start_card(store, start).await?;
            CardRepository::page_after(
                store,
                from.as_ref(),
                filters.account(),
                filters.card(),
                limit,
            )
            .await
        }
        Read::Backward { start } => {
            let from = start_card(store, start).await?;
            CardRepository::page_before(
                store,
                from.as_ref(),
                filters.account(),
                filters.card(),
                limit,
            )
            .await
        }
    }
    .map_err(|error| browse_failed(&error))?;
    let rows: Vec<rules::CardListRow> = entries.into_iter().map(rules::CardListRow::from).collect();
    let lookahead = match plan.lookahead_after(&rows) {
        Some(last) => next_card(store, last).await?,
        None => None,
    };
    Ok(rules::page(&plan, rows, lookahead))
}

/// The rows of one page: the requested `page_size`, or `DEFAULT_PAGE_SIZE` (7, COCRDLIC-017).
fn page_rows(requested: Option<i64>) -> Result<usize, ServiceError<ListFailure>> {
    // TRUNCATION: checked, never lossy. The default is 7 and the validated request 1 to 100, so
    // both conversions hold every value they meet; a refusal is an internal failure, never a
    // wrapped count.
    u32::try_from(rules::DEFAULT_PAGE_SIZE)
        .ok()
        .map(|default| resolve_page_size(requested, default))
        .and_then(|rows| usize::try_from(rows).ok())
        .ok_or(ServiceError::Internal)
}

/// The row bound of the plan's read, as the repository binds it (Bounded Pagination).
fn row_limit(plan: &ListPlan) -> Result<i64, ServiceError<ListFailure>> {
    // TRUNCATION: checked, never lossy. The plan's limit is at most the page size plus one, 101,
    // which `i64` holds; a refusal is an internal failure, never a wrapped bound.
    i64::try_from(plan.limit()).map_err(|_| ServiceError::Internal)
}

/// The card number a browse starts from: the card that the plan's start handle names, or `None`
/// for the key of spaces below every card number (COCRDLIC-017).
///
/// A handle that names no card, as one issued before a reload does, also gives `None`: the
/// forward read restarts from the lowest card number and the backward read finds no card. Such a
/// key is the browse with no position of `HALTED(X-COCRDLIC-H1)` (COCRDLIC-038), whose answer is
/// no parity result.
async fn start_card<R: CardRepository>(
    store: &mut R,
    start: Option<CardRef>,
) -> Result<Option<CardNumber>, ServiceError<ListFailure>> {
    let Some(handle) = start else {
        return Ok(None);
    };
    let entry = CardRepository::find_by_ref(store, handle)
        .await
        .map_err(|error| browse_failed(&error))?;
    Ok(entry.map(|(_, record)| record.num))
}

/// The look-ahead `READNEXT` after the page's last row `last` (COCRDLIC-019): the handle of the
/// next card in card-number order, read without the filters, or `None` at the end of the file.
async fn next_card<R: CardRepository>(
    store: &mut R,
    last: &rules::CardListRow,
) -> Result<Option<CardRef>, ServiceError<ListFailure>> {
    let entries =
        CardRepository::page_after(store, Some(&last.card_num), None, None, LOOKAHEAD_ROWS)
            .await
            .map_err(|error| browse_failed(&error))?;
    Ok(entries
        .into_iter()
        .map(|(card_ref, _)| card_ref)
        .find(|card_ref| *card_ref != last.card_ref))
}

/// A failed read of `CARDDAT`: the file-error text of COCRDLIC-031, a 500. The log line names the
/// SQLSTATE and constraint only, never a key or a filter.
fn browse_failed(error: &PersistenceError) -> ServiceError<ListFailure> {
    tracing::warn!(error = %error, "card list browse failed");
    ServiceError::Rule(ListFailure::file_error())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::future::{Future, ready};
    use std::path::Path;

    use domain::copybook::PicU;
    use domain::copybook::framing::decode_all;
    use domain::online::cocrdlic::{self as rules, CardRef, ListMessage, NextPage};
    use domain::records::card::CardRecord;
    use domain::sensitive::CardNumber;
    use persistence::{CardRepository, PersistenceError};

    use super::browse;
    use crate::dto::PageDirection;
    use crate::dto::cocrdlic::CardListQuery;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// One card as the repository returns it, handle first.
    type Entry = (CardRef, CardRecord);

    /// The bounds and filters of one browse the service ran.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Browse {
        /// `true` for `page_before`.
        backward: bool,
        /// Whether the account filter was bound.
        account: bool,
        /// Whether the card filter was bound.
        card: bool,
        /// The row bound.
        limit: i64,
    }

    /// An in-memory `CARDDAT`, kept in ascending card-number order, that records every browse.
    #[derive(Default)]
    struct MemoryStore {
        entries: Vec<Entry>,
        browses: Vec<Browse>,
        failing: bool,
    }

    impl MemoryStore {
        fn new(entries: Vec<Entry>) -> Self {
            let mut store = Self {
                entries,
                ..Self::default()
            };
            store.sort();
            store
        }

        /// Keeps the entries in ascending card-number order, the KSDS key order.
        fn sort(&mut self) {
            self.entries
                .sort_by(|left, right| left.1.num.cmp(&right.1.num));
        }

        /// The handle the next stored card gets: above every handle held, as an identity column
        /// assigns it.
        fn next_handle(&self) -> i64 {
            self.entries
                .iter()
                .map(|(card_ref, _)| card_ref.get())
                .max()
                .unwrap_or_default()
                .saturating_add(1)
        }

        fn failure(&self) -> Result<(), PersistenceError> {
            if self.failing {
                Err(PersistenceError::Unavailable)
            } else {
                Ok(())
            }
        }

        fn by_num(&self, card: &CardNumber) -> Result<Option<Entry>, PersistenceError> {
            self.failure()?;
            Ok(self.entries.iter().find(|(_, r)| r.num == *card).cloned())
        }

        /// The browse of `queries/cards_page_{forward,backward}.sql` over the vector.
        fn read(
            &mut self,
            backward: bool,
            start: Option<&CardNumber>,
            acct: Option<PicU<11>>,
            card: Option<&CardNumber>,
            limit: i64,
        ) -> Result<Vec<Entry>, PersistenceError> {
            self.browses.push(Browse {
                backward,
                account: acct.is_some(),
                card: card.is_some(),
                limit,
            });
            self.failure()?;
            let rows = usize::try_from(limit).map_err(|_| PersistenceError::Database {
                sqlstate: Some("2201W".to_owned()),
                constraint: None,
            })?;
            let matches = |(_, record): &&Entry| {
                acct.is_none_or(|account| record.acct_id == account)
                    && card.is_none_or(|number| record.num == *number)
            };
            Ok(if backward {
                start.map_or_else(Vec::new, |from| {
                    self.entries
                        .iter()
                        .rev()
                        .filter(|(_, record)| record.num <= *from)
                        .filter(matches)
                        .take(rows)
                        .cloned()
                        .collect()
                })
            } else {
                self.entries
                    .iter()
                    .filter(|(_, record)| start.is_none_or(|from| record.num >= *from))
                    .filter(matches)
                    .take(rows)
                    .cloned()
                    .collect()
            })
        }
    }

    impl CardRepository for MemoryStore {
        fn find_by_id(
            &mut self,
            card: &CardNumber,
        ) -> impl Future<Output = Result<Option<Entry>, PersistenceError>> + Send {
            ready(self.by_num(card))
        }

        fn lock_by_id(
            &mut self,
            card: &CardNumber,
        ) -> impl Future<Output = Result<Option<Entry>, PersistenceError>> + Send {
            ready(self.by_num(card))
        }

        fn find_by_ref(
            &mut self,
            handle: CardRef,
        ) -> impl Future<Output = Result<Option<Entry>, PersistenceError>> + Send {
            ready(self.failure().map(|()| {
                self.entries
                    .iter()
                    .find(|(card_ref, _)| *card_ref == handle)
                    .cloned()
            }))
        }

        fn page_after(
            &mut self,
            start: Option<&CardNumber>,
            acct: Option<PicU<11>>,
            card: Option<&CardNumber>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<Entry>, PersistenceError>> + Send {
            ready(self.read(false, start, acct, card, limit))
        }

        fn page_before(
            &mut self,
            start: Option<&CardNumber>,
            acct: Option<PicU<11>>,
            card: Option<&CardNumber>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<Entry>, PersistenceError>> + Send {
            ready(self.read(true, start, acct, card, limit))
        }

        fn find_all(
            &mut self,
        ) -> impl Future<Output = Result<Vec<Entry>, PersistenceError>> + Send {
            ready(self.failure().map(|()| self.entries.clone()))
        }

        fn insert(
            &mut self,
            record: &CardRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            let outcome = self.by_num(&record.num).and_then(|found| {
                if found.is_some() {
                    return Err(PersistenceError::Duplicate {
                        constraint: Some("cards_pkey".to_owned()),
                    });
                }
                let handle = CardRef::new(self.next_handle());
                self.entries.push((handle, record.clone()));
                self.sort();
                Ok(())
            });
            ready(outcome)
        }

        fn update(
            &mut self,
            record: &CardRecord,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            let outcome = self.failure().map(|()| {
                self.entries
                    .iter_mut()
                    .find(|(_, stored)| stored.num == record.num)
                    .map(|(_, stored)| *stored = record.clone())
                    .is_some()
            });
            ready(outcome)
        }

        fn replace_all(
            &mut self,
            records: &[CardRecord],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            let outcome = self.failure().and_then(|()| {
                let first = self.next_handle();
                self.entries = records
                    .iter()
                    .cloned()
                    .zip(first..)
                    .map(|(record, handle)| (CardRef::new(handle), record))
                    .collect();
                self.sort();
                u64::try_from(self.entries.len())
                    .map_err(|_| PersistenceError::Check { constraint: None })
            });
            ready(outcome)
        }
    }

    /// The 50 cards of `carddata.txt`, card `n` (file line `n`, ascending card numbers) with
    /// handle `n`.
    fn fixture() -> Result<Vec<Entry>, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/carddata.txt");
        Ok(decode_all::<CardRecord>(&std::fs::read(path)?)?
            .into_iter()
            .zip(1_i64..)
            .map(|(line, handle)| (CardRef::new(handle), line.record))
            .collect())
    }

    /// `count` synthetic cards built on the first fixture card: card `n` has handle `n`, the card
    /// number `4000` followed by `n` in twelve digits, and the account `account(n)`.
    fn synthetic(
        count: i64,
        account: impl Fn(i64) -> u64,
    ) -> Result<Vec<Entry>, Box<dyn StdError>> {
        let template = fixture()?
            .into_iter()
            .next()
            .ok_or("carddata.txt holds no card")?
            .1;
        (1..=count)
            .map(|n| {
                let mut record = template.clone();
                record.num = CardNumber::parse(&format!("4000{n:012}"))?;
                record.acct_id = PicU::new(account(n))?;
                Ok((CardRef::new(n), record))
            })
            .collect()
    }

    /// The handles of a page's rows.
    fn handles(page: &rules::CardPage) -> Vec<i64> {
        page.rows().iter().map(|row| row.card_ref.get()).collect()
    }

    /// The next request of the screen: `page`'s paging state echoed with the key pressed.
    fn echo(page: &rules::CardPage, direction: Option<PageDirection>) -> CardListQuery {
        CardListQuery {
            direction,
            page_no: Some(i64::from(page.page_no())),
            first_key: page.first_key().map(CardRef::get),
            last_key: page.last_key().map(CardRef::get),
            has_next_page: Some(page.next_page() == NextPage::Exists),
            last_page_shown: Some(page.last_page() == rules::LastPage::Shown),
            ..CardListQuery::default()
        }
    }

    /// The unfiltered browses of a fresh-start page of `size` rows: the page, then its look-ahead.
    fn page_and_lookahead(limit: i64) -> Vec<Browse> {
        vec![
            Browse {
                backward: false,
                account: false,
                card: false,
                limit,
            },
            Browse {
                backward: false,
                account: false,
                card: false,
                limit: 2,
            },
        ]
    }

    // audit: COCRDLIC-017
    // audit: COCRDLIC-019
    // audit: COCRDLIC-033
    #[tokio::test]
    async fn fresh_start_lists_seven_cards_and_reads_ahead_unfiltered() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let page = browse(&mut store, CardListQuery::default()).await?;
        assert_eq!(handles(&page), (1..=7).collect::<Vec<_>>());
        assert_eq!(page.page_no(), rules::PageNumber::FIRST);
        assert_eq!(page.first_key(), Some(CardRef::new(1)));
        assert_eq!(page.next_page(), NextPage::Exists);
        assert_eq!(page.last_key(), Some(CardRef::new(8)));
        assert_eq!(page.message(), None);
        assert_eq!(
            page.info_message(),
            Some("TYPE S FOR DETAIL, U TO UPDATE ANY RECORD")
        );
        assert_eq!(store.browses, page_and_lookahead(7));
        Ok(())
    }

    // audit: COCRDLIC-018
    // audit: COCRDLIC-023
    // audit: COCRDLIC-027
    // audit: COCRDLIC-028
    #[tokio::test]
    async fn paging_follows_the_echoed_handles() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let first = browse(&mut store, CardListQuery::default()).await?;

        let second = browse(&mut store, echo(&first, Some(PageDirection::Forward))).await?;
        assert_eq!(handles(&second), (8..=14).collect::<Vec<_>>());
        assert_eq!(i64::from(second.page_no()), 2);
        assert_eq!(second.last_key(), Some(CardRef::new(15)));

        let again = browse(&mut store, echo(&second, None)).await?;
        assert_eq!(handles(&again), handles(&second));
        assert_eq!(i64::from(again.page_no()), 2);

        store.browses.clear();
        let back = browse(&mut store, echo(&second, Some(PageDirection::Backward))).await?;
        assert_eq!(handles(&back), (1..=7).collect::<Vec<_>>());
        assert_eq!(back.page_no(), rules::PageNumber::FIRST);
        assert_eq!(back.first_key(), Some(CardRef::new(1)));
        assert_eq!(back.message_kind(), Some(ListMessage::NoPreviousPages));
        assert_eq!(
            store.browses,
            vec![Browse {
                backward: true,
                account: false,
                card: false,
                limit: 8,
            }]
        );
        Ok(())
    }

    // audit: COCRDLIC-010
    // audit: COCRDLIC-020
    #[tokio::test]
    async fn filters_reach_the_query_and_a_short_page_needs_no_look_ahead() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let query = CardListQuery {
            acct_id: Some(account_text(&store, 3)?),
            ..CardListQuery::default()
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(handles(&page), vec![3]);
        assert_eq!(page.next_page(), NextPage::NotExists);
        assert_eq!(page.message_kind(), Some(ListMessage::NoMoreRecords));
        assert_eq!(
            store.browses,
            vec![Browse {
                backward: false,
                account: true,
                card: false,
                limit: 7,
            }]
        );
        Ok(())
    }

    // audit: COCRDLIC-019
    #[tokio::test]
    async fn look_ahead_ignores_the_filters() -> TestResult {
        // Seven cards of account 1, then one of account 2: the filtered page is full, and the
        // look-ahead finds the account-2 card although the filter excludes it.
        let mut store = MemoryStore::new(synthetic(8, |n| if n <= 7 { 1 } else { 2 })?);
        let query = CardListQuery {
            acct_id: Some("00000000001".to_owned()),
            ..CardListQuery::default()
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(handles(&page), (1..=7).collect::<Vec<_>>());
        assert_eq!(page.next_page(), NextPage::Exists);
        assert_eq!(page.last_key(), Some(CardRef::new(8)));
        let lookahead = store.browses.last().copied().ok_or("no look-ahead read")?;
        assert_eq!(
            lookahead,
            Browse {
                backward: false,
                account: false,
                card: false,
                limit: 2,
            }
        );
        Ok(())
    }

    // audit: COCRDLIC-019
    #[tokio::test]
    async fn a_full_last_page_has_no_next_page() -> TestResult {
        let mut store = MemoryStore::new(synthetic(7, |_| 1)?);
        let page = browse(&mut store, CardListQuery::default()).await?;
        assert_eq!(handles(&page), (1..=7).collect::<Vec<_>>());
        assert_eq!(page.next_page(), NextPage::NotExists);
        assert_eq!(page.last_key(), Some(CardRef::new(7)));
        assert_eq!(page.message_kind(), Some(ListMessage::NoMoreRecords));
        Ok(())
    }

    #[tokio::test]
    async fn page_size_bounds_every_read() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let query = CardListQuery {
            page_size: Some(3),
            ..CardListQuery::default()
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(handles(&page), vec![1, 2, 3]);
        assert_eq!(page.last_key(), Some(CardRef::new(4)));
        assert_eq!(store.browses, page_and_lookahead(3));
        Ok(())
    }

    // audit: COCRDLIC-014
    #[tokio::test]
    async fn reread_starts_from_the_first_card() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let first = browse(&mut store, CardListQuery::default()).await?;
        let second = browse(&mut store, echo(&first, Some(PageDirection::Forward))).await?;
        let query = CardListQuery {
            reread: Some(true),
            ..echo(&second, None)
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(handles(&page), (1..=7).collect::<Vec<_>>());
        assert_eq!(i64::from(page.page_no()), 2);
        assert_eq!(page.message(), None);
        Ok(())
    }

    // HALTED(X-COCRDLIC-H2): the evident path of the filter edit, asserted as the replacement's
    // answer and no parity result.
    #[tokio::test]
    async fn a_filter_edit_failure_reads_nothing() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let query = CardListQuery {
            card_num: Some("12345".to_owned()),
            ..CardListQuery::default()
        };
        let failure = match browse(&mut store, query).await {
            Err(ServiceError::Rule(failure)) => failure,
            other => return Err(format!("expected a filter failure, got {other:?}").into()),
        };
        assert_eq!(failure.class(), rules::FailureClass::Input);
        assert_eq!(failure.field(), Some(rules::ListField::Card));
        assert!(store.browses.is_empty());
        Ok(())
    }

    // audit: COCRDLIC-031
    #[tokio::test]
    async fn a_store_failure_is_the_file_error() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        store.failing = true;
        let fresh = browse(&mut store, CardListQuery::default()).await;
        assert_eq!(
            fresh.err(),
            Some(ServiceError::Rule(rules::ListFailure::file_error()))
        );

        // The start-key lookup is a read of the card file too.
        let keyed = CardListQuery {
            first_key: Some(1),
            ..CardListQuery::default()
        };
        assert_eq!(
            browse(&mut store, keyed).await.err(),
            Some(ServiceError::Rule(rules::ListFailure::file_error()))
        );
        Ok(())
    }

    // HALTED(X-COCRDLIC-H1): a key that names no card has no parity result (COCRDLIC-038).
    #[tokio::test]
    async fn an_unknown_handle_restarts_from_the_lowest_card() -> TestResult {
        let mut store = MemoryStore::new(fixture()?);
        let query = CardListQuery {
            first_key: Some(999),
            ..CardListQuery::default()
        };
        let page = browse(&mut store, query).await?;
        assert_eq!(handles(&page), (1..=7).collect::<Vec<_>>());
        Ok(())
    }

    /// The account of fixture card `handle`, as the eleven digits a screen filter types.
    fn account_text(store: &MemoryStore, handle: i64) -> Result<String, Box<dyn StdError>> {
        let (_, record) = store
            .entries
            .iter()
            .find(|(card_ref, _)| card_ref.get() == handle)
            .ok_or("no such fixture card")?;
        Ok(format!("{:011}", u64::from(record.acct_id)))
    }
}
