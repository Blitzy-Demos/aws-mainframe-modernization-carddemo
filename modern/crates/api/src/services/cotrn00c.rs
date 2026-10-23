//! COTRN00C, the transaction list: the service of endpoint 11, `GET /api/v1/transactions`
//! (transaction `CT00`, map `COTRN0A`; AAP 0.3.4.1). Only `api::handlers::cotrn00c::list` calls
//! it (Thin Request Handlers).
//!
//! It ports the browse of `TRANSACT` that `PROCESS-PAGE-FORWARD` and `PROCESS-PAGE-BACKWARD`
//! run: `STARTBR` (`COTRN00C.cbl:591-619`), the `READNEXT` loop (`COTRN00C.cbl:624-653`) or the
//! `READPREV` loop (`COTRN00C.cbl:658-687`), and `ENDBR` (`COTRN00C.cbl:692-696`). Every rule
//! belongs to `domain::online::cotrn00c`; this module only carries a request to it and reads
//! the table it asks for:
//!
//! 1. [`cotrn00c::plan`] takes the key, the raw search text, the echoed paging state and the page
//!    size. It edits the search value (COTRN00C-011 to COTRN00C-013, COTRN00C-040), answers PF7
//!    at the top and PF8 at the bottom without a read (COTRN00C-021, COTRN00C-022), and
//!    otherwise names the browse: direction, start key, skip read and page size (COTRN00C-003,
//!    COTRN00C-004, COTRN00C-014, COTRN00C-015, COTRN00C-023).
//! 2. The browse is one page query through [`TransactionRepository`]: `page_after` ascending
//!    from the start key for a forward page, `page_before` descending to it for a backward one,
//!    both inclusive of the key where `STARTBR` positions.
//! 3. Each record becomes a list row through [`cotrn00c::row_from`] (COTRN00C-028 to
//!    COTRN00C-034), and [`cotrn00c::page`] decides the rows shown, the keys, the page number,
//!    the next-page flag and the message from the records in the order read (COTRN00C-014 to
//!    COTRN00C-020, COTRN00C-024 to COTRN00C-026).
//!
//! A page query that fails is answered with [`cotrn00c::lookup_failure`], 500 `Unable to lookup
//! transaction...` with no rows (COTRN00C-027). One query returns the whole page or fails, so the
//! partial page a failed `READNEXT` leaves in the COBOL has no counterpart (X-COTRN00C-7).
//!
//! # Rules
//!
//! - Bounded Pagination: the page size is the DTO's resolved `page_size` (1 to 100, default the
//!   10 rows of `COTRN0A`), and the query's `LIMIT` is the domain's [`Browse::limit`]: the rows,
//!   the read-ahead record and the skipped record, so at most 102. Only the two queries of
//!   `queries/transactions_page_forward.sql` and `queries/transactions_page_backward.sql` are
//!   issued, both served by the primary key on `tran_id`.
//! - Cardholder Data Redaction: the records read hold card numbers, and no record or row is
//!   logged. The one event this module emits, on a failed read, carries only the
//!   [`persistence::PersistenceError`] display: an SQLSTATE and a constraint name, never a value.
//! - Checked Integer Narrowing: the limit reaches the repository's `i64` through a checked
//!   conversion.

use domain::online::cotrn00c::{self, Browse, Direction, Plan, TranListFailure, TranPage};
use persistence::TransactionRepository;
use tracing::callsite::DefaultCallsite;
use tracing::field::{Value, display};
use tracing::metadata::Kind;
use tracing::{Event, Level, Metadata};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cotrn00c::TranListQuery;
use crate::error::ServiceError;

/// Endpoint 11: the page, or the answer without a read, that the request `query` gets
/// (COTRN00C-003, COTRN00C-004, COTRN00C-011 to COTRN00C-027).
///
/// `query` carries the key sent (ENTER, PF7, PF8 or the transfer key of a first entry), the
/// search value and the paging state the previous response returned (X-COTRN00C-1). The state
/// grants nothing: every page is re-read from `transactions`. The signed-on user only guards the
/// endpoint; the program reads the same table for every user.
///
/// # Errors
///
/// [`ServiceError::Rule`] with the domain failure:
/// - 400 `Tran ID must be Numeric ...` on field `start_tran_id` for an ENTER whose search value
///   is neither blank nor 16 digits (COTRN00C-013; the replacement's answer to COTRN00C-040);
/// - 500 `Unable to lookup transaction...` when the page query fails (COTRN00C-027), and on the
///   halted browse paths the domain answers that way (COTRN00C-020).
///
/// [`ServiceError::Internal`] if the domain's page limit did not fit the repository's `i64`,
/// which the limit of at most 102 records rules out.
pub(crate) async fn list(
    state: &AppState,
    _user: &SignedOnUser,
    query: TranListQuery,
) -> Result<TranPage, ServiceError<TranListFailure>> {
    let mut store = state.store.clone();
    respond(&mut store, query).await
}

/// The answer to `query` over the `transactions` that `repo` reads: the work of [`list`] over
/// any implementation of the repository, so that a test double can stand in for the pool.
async fn respond<R: TransactionRepository>(
    repo: &mut R,
    query: TranListQuery,
) -> Result<TranPage, ServiceError<TranListFailure>> {
    let key = query.key();
    let echoed = query.state();
    let page_size = query.page_size();
    let TranListQuery { start_tran_id, .. } = query;
    match cotrn00c::plan(key, start_tran_id.as_deref(), echoed, page_size)
        .map_err(ServiceError::Rule)?
    {
        Plan::Answer(page) => Ok(page),
        Plan::Read(browse) => read_page(repo, browse).await,
    }
}

/// The page `browse` names, read with one page query: `STARTBR` at the browse key, then the
/// `READNEXT` or `READPREV` loop with its skip and read-ahead records, then `ENDBR`
/// (COTRN00C-014, COTRN00C-015, COTRN00C-023).
async fn read_page<R: TransactionRepository>(
    repo: &mut R,
    browse: Browse,
) -> Result<TranPage, ServiceError<TranListFailure>> {
    // The limit is the domain's, not `page_size + 1`: a PF7, PF8 or PF5-entry browse also reads
    // the record its skip read drops (COTRN00C-004, COTRN00C-015, COTRN00C-023).
    // TRUNCATION: checked, never lossy: `Browse::limit` is at most `MAX_PAGE_SIZE + 2`, which
    // `i64` holds; a conversion that failed would be an internal failure, not a shorter page.
    let limit = i64::try_from(browse.limit()).map_err(|_| ServiceError::Internal)?;
    let start = browse.start().query_key();
    let read = match browse.direction() {
        Direction::Forward => TransactionRepository::page_after(repo, &start, limit).await,
        Direction::Backward => TransactionRepository::page_before(repo, &start, limit).await,
    };
    match read {
        Ok(records) => {
            let rows = records.iter().map(cotrn00c::row_from).collect();
            cotrn00c::page(browse, rows).map_err(ServiceError::Rule)
        }
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
            // The COBOL DISPLAYs `RESP` and `REAS` to the CICS log before it moves its message
            // (COTRN00C-027); the replacement logs the store error, which names an SQLSTATE and a
            // constraint at most and never a stored value.
            if runtime::telemetry::event_enabled(&EVENT) {
                Event::dispatch(
                    &EVENT_METADATA,
                    &EVENT_METADATA.fields().value_set_all(&[
                        Some(&"transaction page read failed" as &dyn Value),
                        Some(&display(&error) as &dyn Value),
                    ]),
                );
            }
            Err(ServiceError::Rule(cotrn00c::lookup_failure()))
        }
    }
}

#[cfg(test)]
mod tests {
    use core::future::{Future, ready};

    use axum::http::StatusCode;
    use chrono::{NaiveDate, NaiveDateTime};
    use domain::copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
    use domain::money::{Decimal, Money};
    use domain::online::cotrn00c as rules;
    use domain::records::TransactionRecord;
    use domain::sensitive::CardNumber;
    use persistence::{PersistenceError, TransactionRepository};

    use super::respond;
    use crate::dto::PageDirection;
    use crate::dto::cotrn00c::{TranListEntryKey, TranListQuery, TranListResponse};
    use crate::error::{ScreenError, ServiceError};

    type TestResult = Result<(), String>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries; a runtime that cannot be built
    /// fails the test with the builder's error text.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?
            .block_on(test)
    }

    /// A synthetic card number, in no fixture, that every record carries; no response may.
    const CARD: &str = "4000001234567899";

    /// A repository call the double saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        /// `page_after` from the start key (as text) with the limit.
        After(String, i64),
        /// `page_before` to the start key (as text) with the limit.
        Before(String, i64),
        /// Any other method, by name: the list never calls one.
        Other(&'static str),
    }

    /// `transactions` held in memory in key order, as the KSDS holds them, recording each call;
    /// with `fail` set, every page read fails as the pool does when the database is down.
    struct Table {
        records: Vec<TransactionRecord>,
        fail: bool,
        calls: Vec<Call>,
    }

    impl Table {
        /// The records `1..=count`, each of 50.47.
        fn of(count: u64) -> Result<Self, String> {
            Ok(Self {
                records: (1..=count).map(record).collect::<Result<_, _>>()?,
                fail: false,
                calls: Vec::new(),
            })
        }

        /// The page read `call`: the records at or above `start` ascending, or at or below it
        /// descending, `limit` at most, or the failure of an unreachable database.
        fn page(
            &mut self,
            call: Call,
            start: &PicX<16>,
            limit: i64,
            ascending: bool,
        ) -> Result<Vec<TransactionRecord>, PersistenceError> {
            self.calls.push(call);
            if self.fail {
                return Err(PersistenceError::Unavailable);
            }
            // TRUNCATION: checked, never lossy. `Browse::limit` is at most `MAX_PAGE_SIZE + 2`,
            // which `usize` holds; a negative `LIMIT` is refused as the server refuses it
            // (SQLSTATE 2201W), never wrapped.
            let take = usize::try_from(limit).map_err(|_| PersistenceError::Database {
                sqlstate: Some("2201W".to_owned()),
                constraint: None,
            })?;
            let key = start.as_bytes();
            Ok(if ascending {
                self.records
                    .iter()
                    .filter(|t| t.id.as_bytes() >= key)
                    .take(take)
                    .cloned()
                    .collect()
            } else {
                self.records
                    .iter()
                    .rev()
                    .filter(|t| t.id.as_bytes() <= key)
                    .take(take)
                    .cloned()
                    .collect()
            })
        }

        /// A method the list must never reach, recorded by `name`.
        fn other(&mut self, name: &'static str) {
            self.calls.push(Call::Other(name));
        }
    }

    impl TransactionRepository for Table {
        fn find_by_id(
            &mut self,
            id: &PicX<16>,
        ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send
        {
            self.other("find_by_id");
            ready(Ok(self.records.iter().find(|t| t.id == *id).cloned()))
        }

        fn find_last(
            &mut self,
        ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send
        {
            self.other("find_last");
            ready(Ok(self.records.last().cloned()))
        }

        fn next_id(&mut self) -> impl Future<Output = Result<PicX<16>, PersistenceError>> + Send {
            self.other("next_id");
            ready(Err(PersistenceError::Unavailable))
        }

        fn insert(
            &mut self,
            _record: &TransactionRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.other("insert");
            ready(Err(PersistenceError::Unavailable))
        }

        fn page_after(
            &mut self,
            start: &PicX<16>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            let call = Call::After(text(start), limit);
            ready(self.page(call, start, limit, true))
        }

        fn page_before(
            &mut self,
            start: &PicX<16>,
            limit: i64,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            let call = Call::Before(text(start), limit);
            ready(self.page(call, start, limit, false))
        }

        fn find_all_in_card_order(
            &mut self,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other("find_all_in_card_order");
            ready(Ok(self.records.clone()))
        }

        fn find_processed_between(
            &mut self,
            _start: NaiveDate,
            _end: NaiveDate,
        ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
            self.other("find_processed_between");
            ready(Ok(Vec::new()))
        }
    }

    /// The text of a key, for the recorded calls.
    fn text(key: &PicX<16>) -> String {
        String::from_utf8_lossy(key.as_bytes()).into_owned()
    }

    /// The `Debug` text of `error`, for a test failure message.
    fn shown<E: core::fmt::Debug>(error: E) -> String {
        format!("{error:?}")
    }

    /// The transaction ID `n`, sixteen digits with leading zeros, as text.
    fn id(n: u64) -> String {
        format!("{n:016}")
    }

    /// Transaction `n` of 50.47, carrying [`CARD`].
    fn record(n: u64) -> Result<TransactionRecord, String> {
        let origin: NaiveDateTime = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_micro_opt(13, 5, 9, 120_000))
            .ok_or_else(|| "no such instant".to_owned())?;
        Ok(TransactionRecord {
            id: PicX::from_text(&id(n)).map_err(shown)?,
            type_cd: PicX::from_text("01").map_err(shown)?,
            cat_cd: PicU::new(1).map_err(shown)?,
            source: PicX::from_text("POS TERM").map_err(shown)?,
            desc: PicX::from_text("Purchase").map_err(shown)?,
            amt: Money::new(Decimal::new(5047, 2)).map_err(shown)?,
            merchant_id: PicU::ZERO,
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            card_num: CardNumber::new(PicX::from_text(CARD).map_err(shown)?),
            orig_ts: PicTimestamp::new(Some(
                CobolTimestamp::new(origin, TsForm::Iso).map_err(shown)?,
            )),
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// The response the endpoint sends for `query` over `table`, or the failure.
    async fn send(
        table: &mut Table,
        query: TranListQuery,
    ) -> Result<TranListResponse, ServiceError<rules::TranListFailure>> {
        respond(table, query).await.map(TranListResponse::from)
    }

    /// The 200 for `query` over `table`, or a test failure.
    async fn ok(table: &mut Table, query: TranListQuery) -> Result<TranListResponse, String> {
        send(table, query).await.map_err(shown)
    }

    /// The query a screen sends with `direction` after `response`, its state echoed.
    fn echo(response: &TranListResponse, direction: Option<PageDirection>) -> TranListQuery {
        TranListQuery {
            direction,
            first_key: response.first_key.clone(),
            last_key: response.last_key.clone(),
            // TRUNCATION: checked, never lossy. The page number is PIC 9(08), at most
            // 99,999,999, which `i64` holds; `.ok()` would drop only an impossible value to
            // absent, never wrap it.
            page_no: i64::try_from(response.page_no).ok(),
            has_next_page: Some(response.has_next_page),
            ..TranListQuery::default()
        }
    }

    /// The IDs on the response's rows, row 1 first, `None` for a blank row.
    fn ids_on(response: &TranListResponse) -> Vec<Option<String>> {
        response
            .rows
            .iter()
            .map(|slot| slot.as_ref().map(|row| row.tran_id.clone()))
            .collect()
    }

    /// Rows holding the transactions `ids`, then `blank` blank rows.
    fn shown_ids(ids: impl IntoIterator<Item = u64>, blank: usize) -> Vec<Option<String>> {
        ids.into_iter()
            .map(|n| Some(id(n)))
            .chain(core::iter::repeat_n(None, blank))
            .collect()
    }

    /// The low-values key of a first page, as the page query takes it: sixteen spaces.
    fn lowest() -> String {
        " ".repeat(16)
    }

    // audit: COTRN00C-011
    // audit: COTRN00C-014
    // audit: COTRN00C-016
    #[test]
    fn enter_with_a_blank_search_reads_the_first_page_from_the_lowest_key() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let page = ok(&mut table, TranListQuery::default()).await?;
            assert_eq!(table.calls, vec![Call::After(lowest(), 11)]);
            assert_eq!(ids_on(&page), shown_ids(1..=10, 0));
            assert!(!page.rows_kept);
            assert_eq!(page.first_key, Some(id(1)));
            assert_eq!(page.last_key, Some(id(10)));
            assert_eq!(page.page_no, 1);
            assert!(page.has_next_page);
            assert_eq!(page.message, None);
            Ok(())
        })
    }

    // audit: COTRN00C-015
    // audit: COTRN00C-018
    // audit: COTRN00C-023
    // audit: COTRN00C-024
    // audit: COTRN00C-025
    #[test]
    fn pf8_and_pf7_browse_from_the_echoed_keys_with_a_skip_read() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let first = ok(&mut table, TranListQuery::default()).await?;
            let second = ok(&mut table, echo(&first, Some(PageDirection::Forward))).await?;
            assert_eq!(ids_on(&second), shown_ids(11..=20, 0));
            assert_eq!((second.page_no, second.has_next_page), (2, true));

            let third = ok(&mut table, echo(&second, Some(PageDirection::Forward))).await?;
            assert_eq!(ids_on(&third), shown_ids(21..=25, 5));
            assert_eq!((third.page_no, third.has_next_page), (3, false));
            assert_eq!(third.first_key, Some(id(21)));
            assert_eq!(third.last_key, Some(id(20)));
            assert_eq!(
                third.message.as_deref(),
                Some("You have reached the bottom of the page...")
            );

            let back = ok(&mut table, echo(&third, Some(PageDirection::Backward))).await?;
            assert_eq!(ids_on(&back), shown_ids(11..=20, 0));
            assert_eq!((back.page_no, back.message.as_deref()), (2, None));

            let top = ok(&mut table, echo(&back, Some(PageDirection::Backward))).await?;
            assert_eq!(ids_on(&top), shown_ids(1..=10, 0));
            assert_eq!(top.page_no, 1);
            assert_eq!(
                top.message.as_deref(),
                Some("You have reached the top of the page...")
            );

            assert_eq!(
                table.calls,
                vec![
                    Call::After(lowest(), 11),
                    Call::After(id(10), 12),
                    Call::After(id(20), 12),
                    Call::Before(id(21), 12),
                    Call::Before(id(11), 12),
                ]
            );
            Ok(())
        })
    }

    // audit: COTRN00C-012
    // audit: COTRN00C-018
    #[test]
    fn a_numeric_search_starts_the_page_at_or_above_it() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let query = TranListQuery {
                start_tran_id: Some(id(20)),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, query).await?;
            assert_eq!(table.calls, vec![Call::After(id(20), 11)]);
            assert_eq!(ids_on(&page), shown_ids(20..=25, 4));
            assert_eq!((page.page_no, page.has_next_page), (1, false));
            Ok(())
        })
    }

    // audit: COTRN00C-013
    #[test]
    fn a_non_numeric_search_is_refused_before_any_read() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let query = TranListQuery {
                start_tran_id: Some("12AB".to_owned()),
                ..TranListQuery::default()
            };
            let Err(ServiceError::Rule(failure)) = send(&mut table, query).await else {
                return Err("a non-numeric search must fail its edit".to_owned());
            };
            let error = ScreenError::from(failure);
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            assert_eq!(error.message(), rules::TRAN_ID_NOT_NUMERIC);
            assert_eq!(error.field(), Some("start_tran_id"));
            assert_eq!(table.calls, Vec::new());
            Ok(())
        })
    }

    // audit: COTRN00C-021
    // audit: COTRN00C-022
    #[test]
    fn the_boundary_keys_answer_without_a_read() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let bottom = TranListQuery {
                direction: Some(PageDirection::Forward),
                has_next_page: Some(false),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, bottom).await?;
            assert!(page.rows_kept);
            assert_eq!(page.message.as_deref(), Some(rules::ALREADY_AT_BOTTOM));

            let top = TranListQuery {
                direction: Some(PageDirection::Backward),
                page_no: Some(1),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, top).await?;
            assert!(page.rows_kept);
            assert!(page.has_next_page);
            assert_eq!(page.message.as_deref(), Some(rules::ALREADY_AT_TOP));
            assert_eq!(table.calls, Vec::new());
            Ok(())
        })
    }

    // audit: COTRN00C-027
    #[test]
    fn a_failed_page_read_is_the_lookup_failure() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            table.fail = true;
            let backward = TranListQuery {
                direction: Some(PageDirection::Backward),
                first_key: Some(id(11)),
                page_no: Some(2),
                ..TranListQuery::default()
            };
            for query in [TranListQuery::default(), backward] {
                let Err(ServiceError::Rule(failure)) = send(&mut table, query).await else {
                    return Err("a failed read must answer the lookup failure".to_owned());
                };
                assert_eq!(failure, rules::lookup_failure());
                let error = ScreenError::from(failure);
                assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
                assert_eq!(error.message(), rules::LOOKUP_FAILED);
                assert_eq!(error.field(), None);
            }
            assert_eq!(
                table.calls,
                vec![Call::After(lowest(), 11), Call::Before(id(11), 12)]
            );
            Ok(())
        })
    }

    // audit: COTRN00C-004
    // audit: COTRN00C-014
    #[test]
    fn the_query_limit_follows_the_page_size_and_the_skip_read() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let small = TranListQuery {
                page_size: Some(3),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, small).await?;
            assert_eq!(ids_on(&page), shown_ids(1..=3, 0));
            let next = TranListQuery {
                page_size: Some(3),
                ..echo(&page, Some(PageDirection::Forward))
            };
            let page = ok(&mut table, next).await?;
            assert_eq!(ids_on(&page), shown_ids(4..=6, 0));

            let largest = TranListQuery {
                page_size: Some(100),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, largest).await?;
            assert_eq!(page.rows.len(), 100);

            let by_pf5 = TranListQuery {
                entry_key: Some(TranListEntryKey::Pf5),
                ..TranListQuery::default()
            };
            let page = ok(&mut table, by_pf5).await?;
            assert_eq!(ids_on(&page), shown_ids(2..=11, 0));

            assert_eq!(
                table.calls,
                vec![
                    Call::After(lowest(), 4),
                    Call::After(id(3), 5),
                    Call::After(lowest(), 101),
                    Call::After(lowest(), 12),
                ]
            );
            Ok(())
        })
    }

    // audit: COTRN00C-034
    #[test]
    fn no_row_carries_the_card_number() -> TestResult {
        block_on(async {
            let mut table = Table::of(25)?;
            let page = ok(&mut table, TranListQuery::default()).await?;
            let body = serde_json::to_string(&page).map_err(shown)?;
            assert!(!body.contains(CARD), "the list must not show a card number");
            assert!(!body.contains("7899"), "nor its last four digits");
            Ok(())
        })
    }
}
