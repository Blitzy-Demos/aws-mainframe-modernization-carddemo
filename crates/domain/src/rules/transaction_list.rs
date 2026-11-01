//! `COTRN00C` (CT00): the transaction list, a browse of the live transaction
//! master `TRANSACT` in TRAN-ID order.
//!
//! The program pages through every transaction from a start key: ENTER
//! browses forward from the key typed in `TRNIDIN` (blank: from the first
//! key), PF8 forward from the last key shown, PF7 backward from the first key
//! shown, ten rows a page [app/cbl/COTRN00C.cbl:L146-380]. It has no account
//! filter. `GET /transactions?start_key=&direction=&page_size=&account_id=`
//! serves it through [`list`].
//!
//! # Request mapping
//!
//! The REST request carries no COMMAREA, so the page keys stand for it:
//!
//! - A forward request is ENTER with `start_key` in `TRNIDIN`: `STARTBR` at the
//!   key, `GTEQ` by default, then `READNEXT` (P-COTRN00C-005, P-COTRN00C-006).
//!   `next_key` is the record the look-ahead `READNEXT` found, the first row of
//!   the next page, so a forward request from it shows what PF8 shows after
//!   discarding the last key shown. `None` is `NEXT-PAGE-NO`.
//! - A backward request is PF7 with `start_key` as the first key shown:
//!   `STARTBR` at the key, one `READPREV` discarded, then `READPREV` up to a
//!   page, shown in ascending order (P-COTRN00C-007). `prev_key` is the first
//!   key shown, `None` where PF7 would answer that the top is reached.
//! - A backward request without `start_key` reads the end of the file, as
//!   `STARTBR` at `HIGH-VALUES` then `READPREV` does: the CT02 "copy last
//!   transaction" source and the dashboard's latest rows.
//!
//! Page-boundary texts carry no `WS-ERR-FLG` and come back in
//! [`TransactionListPage::message`] of a successful page (E-54); an empty page
//! is a page with no items, the `STARTBR` `NOTFND` of an empty master (after
//! `batch tranbkp`, E-09) included (E-27). The non-numeric start key and every
//! failed file access set `WS-ERR-FLG` and are a [`DomainError`] on `TRNIDIN`.
//!
//! The row selectors (`SEL0001` to `SEL0010`, P-COTRN00C-002 to
//! P-COTRN00C-004, V-COTRN00C-002) have no REST parameter: the row's view link
//! replaces them (E-54). The `account_id` filter is the REST resource's
//! addition, edited by the API before it reaches this rule (V-COTRN00C-001);
//! parity is measured without it (E-17, P-COTRN00C-015). Each item is the whole
//! record: the row `POPULATE-TRAN-DATA` shows is drawn from its TRAN-ID,
//! TRAN-ORIG-TS, TRAN-DESC and TRAN-AMT, the amount edited by
//! [`PIC_TRAN_AMT_DISPLAY`](crate::format::PIC_TRAN_AMT_DISPLAY)
//! (P-COTRN00C-008).

use crate::entities::Transaction;
use crate::error::DomainError;
use crate::messages;
use crate::numeric::{AccountId, TranId};
use crate::ports::{Browse, Direction, TransactionReader};

/// The rows of the `COTRN00` list: `TRNID01` to `TRNID10`, the page size
/// [`list`] uses when the request names none (P-COTRN00C-006).
pub const SCREEN_ROWS: u16 = 10;

/// The BMS field every `COTRN00C` message puts the cursor on: `TRNIDIN`
/// (`MOVE -1 TO TRNIDINL`).
const TRNIDIN: &str = "TRNIDIN";

/// `TRNIDINI PIC X(16)` [app/cpy-bms/COTRN00.CPY:L66]: the width the
/// `IS NUMERIC` class test covers.
const START_KEY_LEN: usize = 16;

/// The largest page [`list`] reads. A backward page asks for two rows beyond
/// it (the discarded start record and the look-ahead), which must still fit
/// the browse limit.
const MAX_PAGE_ROWS: u16 = u16::MAX - 2;

/// A `GET /transactions` request, its query parameters as the API resolved
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionListInput {
    /// `start_key` as entered: the `TRNIDIN` value of ENTER and PF8, or the
    /// first key shown for PF7. [`edit_start_key`] edits it.
    pub start_key: Option<String>,
    /// [`Direction::Forward`] for ENTER and PF8, [`Direction::Backward`] for
    /// PF7.
    pub direction: Direction,
    /// The most rows the page holds; the API defaults it to [`SCREEN_ROWS`].
    /// Zero is read as one row.
    pub page_size: u16,
    /// Only transactions whose card the cross-reference maps to this account,
    /// when given: the filter the REST resource adds (E-17).
    pub account_id: Option<AccountId>,
}

/// One page of the transaction list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionListPage {
    /// The transactions shown, in ascending TRAN-ID order whatever the
    /// direction: only the records read, never blank rows (P-COTRN00C-016).
    pub items: Vec<Transaction>,
    /// The start key of the next forward page, `None` when no record follows
    /// the page (`NEXT-PAGE-NO`).
    pub next_key: Option<TranId>,
    /// The start key of the previous backward page, `None` when the page is
    /// known to begin at the top of the file.
    pub prev_key: Option<TranId>,
    /// The page-boundary text the COBOL sends with the page, if any.
    pub message: Option<&'static str>,
}

impl TransactionListPage {
    /// A page with no rows and no keys, carrying `message`.
    const fn empty(message: &'static str) -> Self {
        Self {
            items: Vec::new(),
            next_key: None,
            prev_key: None,
            message: Some(message),
        }
    }
}

/// COTRN00C PROCESS-ENTER-KEY — P-COTRN00C-005, V-COTRN00C-004
/// [app/cbl/COTRN00C.cbl:L206-219]: the start key edit.
///
/// No text, an empty text, or a text of spaces only (`SPACES`) or of NUL
/// characters only (`LOW-VALUES`) starts the browse at `LOW-VALUES`, the
/// lowest key, and is `Ok(None)`. Sixteen ASCII digits are the key itself.
///
/// # Errors
///
/// Any other text fails the `IS NUMERIC` class test of `TRNIDINI PIC X(16)`:
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with
/// `"Tran ID must be Numeric ..."` on `TRNIDIN`. A shorter digit string fails
/// too, because the field holds it padded to 16 bytes.
pub fn edit_start_key(raw: Option<&str>) -> Result<Option<TranId>, DomainError> {
    let Some(text) = raw else {
        return Ok(None);
    };
    if text.bytes().all(|byte| byte == b' ') || text.bytes().all(|byte| byte == 0) {
        return Ok(None);
    }
    if text.len() == START_KEY_LEN && text.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(Some(TranId::move_from(text)))
    } else {
        Err(DomainError::invalid(messages::TRAN_ID_MUST_BE_NUMERIC).with_field(TRNIDIN))
    }
}

/// COTRN00C PROCESS-ENTER-KEY, PROCESS-PF7-KEY, PROCESS-PF8-KEY —
/// P-COTRN00C-005 to P-COTRN00C-007, P-COTRN00C-009, P-COTRN00C-011 to
/// P-COTRN00C-016, V-COTRN00C-004 to V-COTRN00C-013
/// [app/cbl/COTRN00C.cbl:L146-380]: one page of the transaction list.
///
/// The start key is edited first, so a rejected key reads nothing. A forward
/// request pages as ENTER does from the key, a backward one as PF7 does from
/// it, and a backward one without a key reads the end of the file (see the
/// module documentation). The account filter, when given, applies to every
/// read, so the pages are those of the filtered file; without it the result
/// is the `COTRN00C` browse unchanged.
///
/// # Errors
///
/// - The start key edit of [`edit_start_key`] (V-COTRN00C-004).
/// - Any failure of the browse, `RESP` other than `NORMAL`, `NOTFND` or
///   `ENDFILE`:
///   [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with
///   `"Unable to lookup transaction..."` on `TRNIDIN` (V-COTRN00C-006,
///   V-COTRN00C-008, V-COTRN00C-011, V-COTRN00C-013).
pub async fn list<S: TransactionReader>(
    store: &mut S,
    input: TransactionListInput,
) -> Result<TransactionListPage, DomainError> {
    let start = edit_start_key(input.start_key.as_deref())?;
    let rows = input.page_size.clamp(1, MAX_PAGE_ROWS);
    match (input.direction, start) {
        (Direction::Forward, start) => page_forward(store, start, rows, input.account_id).await,
        (Direction::Backward, Some(key)) => page_backward(store, key, rows, input.account_id).await,
        (Direction::Backward, None) => page_from_end(store, rows, input.account_id).await,
    }
}

/// COTRN00C STARTBR-TRANSACT-FILE, READNEXT-TRANSACT-FILE,
/// READPREV-TRANSACT-FILE `WHEN OTHER` — P-COTRN00C-012 to P-COTRN00C-014,
/// V-COTRN00C-006, V-COTRN00C-008, V-COTRN00C-011, V-COTRN00C-013
/// [app/cbl/COTRN00C.cbl:L612-618]: the failed browse.
fn lookup_failed() -> DomainError {
    DomainError::unexpected(messages::UNABLE_TO_LOOKUP_TRANSACTION_COTRN00C).with_field(TRNIDIN)
}

/// One browse of the live master, a failure mapped to [`lookup_failed`].
async fn browse<S: TransactionReader>(
    store: &mut S,
    start_key: Option<TranId>,
    direction: Direction,
    limit: u16,
    account_id: Option<AccountId>,
) -> Result<Vec<Transaction>, DomainError> {
    let browse = Browse {
        start_key,
        direction,
        limit,
    };
    store
        .browse_transactions(browse, account_id)
        .await
        .map_err(|_| lookup_failed())
}

/// COTRN00C PROCESS-PAGE-FORWARD — P-COTRN00C-006, P-COTRN00C-011,
/// P-COTRN00C-013, V-COTRN00C-005, V-COTRN00C-007
/// [app/cbl/COTRN00C.cbl:L279-328]: ENTER from `start` (`LOW-VALUES` when
/// `None`), up to `rows` `READNEXT`s and the look-ahead one.
async fn page_forward<S: TransactionReader>(
    store: &mut S,
    start: Option<TranId>,
    rows: u16,
    account_id: Option<AccountId>,
) -> Result<TransactionListPage, DomainError> {
    let from_lowest_key = start.is_none();
    let mut items = browse(store, start, Direction::Forward, rows + 1, account_id).await?;
    if items.is_empty() {
        // STARTBR NOTFND: no key at or after the start key, the empty master
        // included. The COBOL then ends a browse that never started and
        // abends; the replacement answers the empty page with the text sent.
        return Ok(TransactionListPage::empty(
            messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
        ));
    }
    let page_len = usize::from(rows);
    let next_key = items.get(page_len).map(|tran| tran.tran_id);
    items.truncate(page_len);
    // A start key equal to the highest TRAN-ID is ENTER with that key: it
    // shows the record and the end-of-file text. The PF8-without-next-page
    // text ("You are already at the bottom of the page...", V-COTRN00C-003)
    // is not substituted here, because a client following `next_key` to a
    // final page of one record sends this same request and must see it;
    // `next_key: None` is how a page says that no further page exists.
    let message = next_key
        .is_none()
        .then_some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE);
    // ENTER leaves the page number at 1, so PF7 from a page read from the
    // lowest key answers "already at the top": that page offers no previous
    // key. A page read from a typed key may have records before it.
    let prev_key = if from_lowest_key {
        None
    } else {
        items.first().map(|tran| tran.tran_id)
    };
    Ok(TransactionListPage {
        items,
        next_key,
        prev_key,
        message,
    })
}

/// COTRN00C PROCESS-PF7-KEY, PROCESS-PAGE-BACKWARD — P-COTRN00C-007,
/// P-COTRN00C-009, P-COTRN00C-011, P-COTRN00C-014, V-COTRN00C-009,
/// V-COTRN00C-010, V-COTRN00C-012 [app/cbl/COTRN00C.cbl:L234-252]
/// [app/cbl/COTRN00C.cbl:L333-376]: PF7 with `key` as the first key shown.
///
/// `STARTBR` positions `GTEQ` on `key`; the first `READPREV` returns the
/// record there and is discarded, then up to `rows` `READPREV`s fill the page
/// from its bottom row up and one more looks ahead.
async fn page_backward<S: TransactionReader>(
    store: &mut S,
    key: TranId,
    rows: u16,
    account_id: Option<AccountId>,
) -> Result<TransactionListPage, DomainError> {
    let mut before = browse(store, Some(key), Direction::Backward, rows + 2, account_id).await?;
    let anchor = if before.first().is_some_and(|tran| tran.tran_id == key) {
        before.remove(0);
        key
    } else {
        // The start key names no record, so the browse is positioned on the
        // first key above it, which the discarded READPREV returns; every
        // record read before it lies below the start key either way. A start
        // key above the highest key is the STARTBR NOTFND, answered with the
        // empty page and the text sent, as the forward page answers it.
        let above = browse(store, Some(key), Direction::Forward, 1, account_id).await?;
        let Some(first_above) = above.first() else {
            return Ok(TransactionListPage::empty(
                messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
            ));
        };
        first_above.tran_id
    };
    if before.is_empty() {
        // Nothing lies before the first key shown: the page-1 answer of
        // PROCESS-PF7-KEY, which reads nothing. PF7 sets NEXT-PAGE-YES before
        // that test, so the page from the start key stays reachable forward.
        return Ok(TransactionListPage {
            items: Vec::new(),
            next_key: Some(anchor),
            prev_key: None,
            message: Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
        });
    }
    Ok(backward_page(before, rows, Some(anchor)))
}

/// `STARTBR` at `HIGH-VALUES` then `READPREV`, as `COTRN02C` reads the
/// highest key for its "copy last transaction" (P-COTRN02C-026), with the
/// `COTRN00C` page rules of [`backward_page`]: the latest `rows` transactions.
///
/// `STARTBR` on an empty master is `NOTFND`, answered as P-COTRN00C-011 and
/// E-27 decide for the list: the empty page with
/// `"You are at the top of the page..."`.
async fn page_from_end<S: TransactionReader>(
    store: &mut S,
    rows: u16,
    account_id: Option<AccountId>,
) -> Result<TransactionListPage, DomainError> {
    let before = browse(store, None, Direction::Backward, rows + 1, account_id).await?;
    if before.is_empty() {
        return Ok(TransactionListPage::empty(
            messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
        ));
    }
    Ok(backward_page(before, rows, None))
}

/// COTRN00C PROCESS-PAGE-BACKWARD — P-COTRN00C-007, P-COTRN00C-014,
/// V-COTRN00C-012 [app/cbl/COTRN00C.cbl:L349-369]: the page of the records
/// `before`, read descending, up to `rows` of them and one look-ahead.
///
/// When the `READPREV` loop or the look-ahead read reaches `ENDFILE`, the page
/// carries `"You have reached the top of the page..."` and no previous key;
/// otherwise the previous page starts at the first key shown.
fn backward_page(
    mut before: Vec<Transaction>,
    rows: u16,
    next_key: Option<TranId>,
) -> TransactionListPage {
    let page_len = usize::from(rows);
    let more_before = before.len() > page_len;
    before.truncate(page_len);
    // Rows are filled from row 10 up, so the screen shows them ascending.
    before.reverse();
    let (prev_key, message) = if more_before {
        (before.first().map(|tran| tran.tran_id), None)
    } else {
        (None, Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE))
    };
    TransactionListPage {
        items: before,
        next_key,
        prev_key,
        message,
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;

    use super::{SCREEN_ROWS, TransactionListInput, TransactionListPage, edit_start_key, list};
    use crate::entities::{CardXref, NewTransaction};
    use crate::error::{DomainError, ErrorKind};
    use crate::messages;
    use crate::numeric::{
        AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode, TranId,
        TranTypeCode,
    };
    use crate::ports::{Direction, PortError};
    use crate::rules::test_support::{FakeStore, block_on};

    // Account 1 holds card A, account 2 card B.
    const CARD_A: &str = "4111000000000001";
    const CARD_B: &str = "4111000000000002";

    /// The TRAN-ID `n`, zero-padded to 16 digits as CT02 allocates it.
    fn key(n: u32) -> TranId {
        TranId::move_from(&format!("{n:016}"))
    }

    fn keys(range: std::ops::RangeInclusive<u32>) -> Vec<TranId> {
        range.map(key).collect()
    }

    fn account(id: u64) -> AccountId {
        AccountId::new(id).expect("an account number of at most 11 digits")
    }

    fn new_tran(n: u32, card: &str) -> NewTransaction {
        NewTransaction {
            tran_id: key(n),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: TranCategoryCode::new(1).expect("a category of at most 4 digits"),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: Money::from_cents(5_047).expect("50.47 fits S9(09)V99"),
            tran_merchant_id: MerchantId::new(800_000_000).expect("a nine-digit merchant"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from(card),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from("2022-07-01-12.00.00.000000"),
        }
    }

    fn xref(card: &str, account_id: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(card),
            xref_cust_id: CustomerId::new(1).expect("a customer number of at most 9 digits"),
            xref_acct_id: account(account_id),
        }
    }

    /// The live master holding TRAN-IDs `1..=count`; every third one is on
    /// card B (account 2), the others on card A (account 1).
    fn store(count: u32) -> FakeStore {
        (1..=count).fold(
            FakeStore::default()
                .with_xref(xref(CARD_A, 1))
                .with_xref(xref(CARD_B, 2)),
            |store, n| {
                let card = if n % 3 == 0 { CARD_B } else { CARD_A };
                store.with_transaction(new_tran(n, card))
            },
        )
    }

    fn input(start: Option<TranId>, direction: Direction, page_size: u16) -> TransactionListInput {
        TransactionListInput {
            start_key: start.map(|start| start.to_string()),
            direction,
            page_size,
            account_id: None,
        }
    }

    fn page(store: &mut FakeStore, input: TransactionListInput) -> TransactionListPage {
        block_on(list(store, input)).expect("a page of the live master")
    }

    fn item_keys(page: &TransactionListPage) -> Vec<TranId> {
        page.items.iter().map(|tran| tran.tran_id).collect()
    }

    /// Requires `future` to be `Send`, as the multi-threaded API runtime does.
    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    fn tran_id_not_numeric() -> DomainError {
        DomainError::invalid(messages::TRAN_ID_MUST_BE_NUMERIC).with_field("TRNIDIN")
    }

    /// The outcome of [`edit_start_key`].
    type Edited = Result<Option<TranId>, DomainError>;

    #[test]
    fn start_key_edit_accepts_blank_and_sixteen_digits_only() {
        // P-COTRN00C-005, V-COTRN00C-004
        let cases: [(Option<&str>, Edited); 11] = [
            (None, Ok(None)),
            (Some(""), Ok(None)),
            (Some("                "), Ok(None)),
            (Some("\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"), Ok(None)),
            (Some("0000000000683580"), Ok(Some(key(683_580)))),
            (Some("000000000068358X"), Err(tran_id_not_numeric())),
            (Some("683580"), Err(tran_id_not_numeric())),
            (Some(" 000000000068358"), Err(tran_id_not_numeric())),
            (Some("00000000000000005"), Err(tran_id_not_numeric())),
            (Some("-000000000068358"), Err(tran_id_not_numeric())),
            (Some("        \0\0\0\0\0\0\0\0"), Err(tran_id_not_numeric())),
        ];
        for (raw, expected) in cases {
            assert_eq!(edit_start_key(raw), expected, "{raw:?}");
        }
        let error = edit_start_key(Some("ABC")).expect_err("a non-numeric key");
        assert_eq!(error.kind, ErrorKind::Invalid);
        assert_eq!(error.first_field(), Some("TRNIDIN"));
    }

    #[test]
    fn a_rejected_start_key_reads_nothing() {
        // V-COTRN00C-004: WS-ERR-FLG is set and nothing is browsed.
        for direction in [Direction::Forward, Direction::Backward] {
            let mut store = store(3);
            let request = TransactionListInput {
                start_key: Some("000000000068358X".to_owned()),
                direction,
                page_size: SCREEN_ROWS,
                account_id: None,
            };
            assert_eq!(
                block_on(list(&mut store, request)),
                Err(tran_id_not_numeric())
            );
            assert!(store.calls.is_empty(), "{direction:?}: {:?}", store.calls);
        }
    }

    #[test]
    fn first_page_holds_ten_rows_and_the_look_ahead_key() {
        // P-COTRN00C-006: ENTER from LOW-VALUES, page_size 10, next page when
        // one more READNEXT succeeds.
        let mut store = store(23);
        let first = page(&mut store, input(None, Direction::Forward, SCREEN_ROWS));
        assert_eq!(item_keys(&first), keys(1..=10));
        assert_eq!(first.next_key, Some(key(11)));
        assert_eq!(first.prev_key, None);
        assert_eq!(first.message, None);
        assert_eq!(store.calls, ["browse_transactions"]);
    }

    /// Follows `next_key` from the first page until it is `None`, returning
    /// each page.
    fn forward_pages(store: &mut FakeStore, page_size: u16) -> Vec<TransactionListPage> {
        let mut pages = vec![page(store, input(None, Direction::Forward, page_size))];
        while let Some(next) = pages.last().and_then(|last| last.next_key) {
            assert!(pages.len() < 30, "paging never ended");
            pages.push(page(
                store,
                input(Some(next), Direction::Forward, page_size),
            ));
        }
        pages
    }

    #[test]
    fn forward_pages_cover_the_file_once_and_end_at_the_bottom() {
        // P-COTRN00C-006, P-COTRN00C-013, V-COTRN00C-007: a short last page
        // ends in the READNEXT loop, a full one at the look-ahead READNEXT;
        // both carry the ENDFILE text. A last page of one record (21) is
        // reached through the previous page's next_key (V-COTRN00C-003 is not
        // substituted for it).
        for (count, page_size, page_count) in [
            (23, SCREEN_ROWS, 3),
            (20, SCREEN_ROWS, 2),
            (21, SCREEN_ROWS, 3),
            (7, SCREEN_ROWS, 1),
            (5, 2, 3),
            (3, 0, 3),
        ] {
            let mut store = store(count);
            let pages = forward_pages(&mut store, page_size);
            assert_eq!(pages.len(), page_count, "{count} rows by {page_size}");
            let shown: Vec<TranId> = pages.iter().flat_map(item_keys).collect();
            assert_eq!(shown, keys(1..=count), "{count} rows by {page_size}");
            let (last, earlier) = pages.split_last().expect("at least one page");
            assert_eq!(
                last.message,
                Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE)
            );
            assert_eq!(last.next_key, None);
            for earlier in earlier {
                assert_eq!(earlier.message, None);
            }
            for later in &pages[1..] {
                assert_eq!(later.prev_key, later.items.first().map(|tran| tran.tran_id));
            }
        }
    }

    #[test]
    fn forward_from_a_key_starts_at_the_first_key_at_or_after_it() {
        // P-COTRN00C-006: STARTBR GTEQ. The highest key is shown with the
        // ENDFILE text, as ENTER with that key shows it.
        let mut store = store(23);
        let cases = [
            (key(5), keys(5..=14), Some(key(15))),
            (
                TranId::move_from("0000000000000000"),
                keys(1..=10),
                Some(key(11)),
            ),
            (key(23), keys(23..=23), None),
        ];
        for (start, expected, next_key) in cases {
            let shown = page(
                &mut store,
                input(Some(start), Direction::Forward, SCREEN_ROWS),
            );
            assert_eq!(item_keys(&shown), expected, "{start}");
            assert_eq!(shown.next_key, next_key, "{start}");
            assert_eq!(shown.prev_key, expected.first().copied(), "{start}");
        }
        let mut gaps = FakeStore::default()
            .with_transaction(new_tran(10, CARD_A))
            .with_transaction(new_tran(20, CARD_A));
        let between = page(
            &mut gaps,
            input(Some(key(15)), Direction::Forward, SCREEN_ROWS),
        );
        assert_eq!(item_keys(&between), [key(20)]);
        assert_eq!(
            between.message,
            Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE)
        );
    }

    #[test]
    fn backward_from_the_first_key_shown_returns_the_page_before() {
        // P-COTRN00C-007: the READPREV at the start key is discarded, rows
        // 10 down to 1 are filled, the display stays ascending.
        // P-COTRN00C-014, V-COTRN00C-012: ENDFILE in the loop or at the
        // look-ahead READPREV carries the top-of-file text.
        let cases = [
            (key(21), keys(11..=20), Some(key(11)), None),
            (
                key(11),
                keys(1..=10),
                None,
                Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
            ),
            (
                key(3),
                keys(1..=2),
                None,
                Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
            ),
        ];
        let mut store = store(23);
        for (start, expected, prev_key, message) in cases {
            let shown = page(
                &mut store,
                input(Some(start), Direction::Backward, SCREEN_ROWS),
            );
            assert_eq!(item_keys(&shown), expected, "{start}");
            assert_eq!(shown.next_key, Some(start), "{start}");
            assert_eq!(shown.prev_key, prev_key, "{start}");
            assert_eq!(shown.message, message, "{start}");
        }
        assert_eq!(store.calls, ["browse_transactions"; 3]);

        // Page 2 and back: the pages a client walks with prev_key and
        // next_key repeat no row.
        let second = page(
            &mut store,
            input(Some(key(11)), Direction::Forward, SCREEN_ROWS),
        );
        let back = page(
            &mut store,
            input(second.prev_key, Direction::Backward, SCREEN_ROWS),
        );
        assert_eq!(item_keys(&back), keys(1..=10));
        assert_eq!(back.next_key, Some(key(11)));
    }

    #[test]
    fn backward_from_the_lowest_key_is_already_at_the_top() {
        // P-COTRN00C-009, V-COTRN00C-009: nothing lies before the first key
        // shown; nothing is read into the page.
        let mut store = store(23);
        for start in [key(1), TranId::move_from("0000000000000000")] {
            let shown = page(
                &mut store,
                input(Some(start), Direction::Backward, SCREEN_ROWS),
            );
            assert_eq!(
                shown,
                TransactionListPage {
                    items: Vec::new(),
                    next_key: Some(key(1)),
                    prev_key: None,
                    message: Some(messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE),
                },
                "{start}"
            );
        }
    }

    #[test]
    fn backward_from_a_key_between_records_reads_the_records_below_it() {
        // P-COTRN00C-007: STARTBR GTEQ positions on the next key, whose
        // discarded READPREV leaves the records below the start key.
        let mut store = FakeStore::default()
            .with_transaction(new_tran(10, CARD_A))
            .with_transaction(new_tran(20, CARD_A))
            .with_transaction(new_tran(30, CARD_A));
        let shown = page(
            &mut store,
            input(Some(key(25)), Direction::Backward, SCREEN_ROWS),
        );
        assert_eq!(item_keys(&shown), [key(10), key(20)]);
        assert_eq!(shown.next_key, Some(key(30)));
        assert_eq!(shown.prev_key, None);
        assert_eq!(
            shown.message,
            Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE)
        );
        assert_eq!(store.calls, ["browse_transactions"; 2]);
    }

    #[test]
    fn no_key_at_or_after_the_start_key_is_an_empty_page() {
        // P-COTRN00C-011, V-COTRN00C-005, V-COTRN00C-010, E-27: STARTBR
        // NOTFND in either direction.
        let above = TranId::move_from("9999999999999999");
        for direction in [Direction::Forward, Direction::Backward] {
            let mut store = store(23);
            assert_eq!(
                page(&mut store, input(Some(above), direction, SCREEN_ROWS)),
                TransactionListPage::empty(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
                "{direction:?}"
            );
        }
    }

    #[test]
    fn an_empty_master_answers_the_empty_page() {
        // E-09, E-27, P-COTRN00C-011: after batch tranbkp the live master is
        // empty; every request answers 200 with no items.
        for (start, direction) in [
            (None, Direction::Forward),
            (None, Direction::Backward),
            (Some(key(1)), Direction::Forward),
            (Some(key(1)), Direction::Backward),
        ] {
            let mut store = FakeStore::default();
            assert_eq!(
                page(&mut store, input(start, direction, SCREEN_ROWS)),
                TransactionListPage::empty(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE),
                "{start:?} {direction:?}"
            );
        }
    }

    #[test]
    fn backward_without_a_key_reads_the_highest_keys() {
        // CT02 PF5 copy-last (P-COTRN02C-026): page_size 1 is the highest
        // TRAN-ID; the end of the file has no next page.
        let cases = [
            (23, 1, keys(23..=23), Some(key(23)), None),
            (23, 5, keys(19..=23), Some(key(19)), None),
            (
                1,
                1,
                keys(1..=1),
                None,
                Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
            ),
            (
                4,
                5,
                keys(1..=4),
                None,
                Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
            ),
        ];
        for (count, page_size, expected, prev_key, message) in cases {
            let mut store = store(count);
            let shown = page(&mut store, input(None, Direction::Backward, page_size));
            assert_eq!(item_keys(&shown), expected, "{count} by {page_size}");
            assert_eq!(shown.next_key, None, "{count} by {page_size}");
            assert_eq!(shown.prev_key, prev_key, "{count} by {page_size}");
            assert_eq!(shown.message, message, "{count} by {page_size}");
        }
    }

    /// The keys `1..=count` of account 2's card B: every third one.
    fn account_two_keys(count: u32) -> Vec<TranId> {
        (1..=count).filter(|n| n % 3 == 0).map(key).collect()
    }

    #[test]
    fn the_account_filter_pages_the_account_rows_only() {
        // E-17, P-COTRN00C-015: the added filter; rows of other accounts are
        // skipped and not counted, the unfiltered browse is unchanged.
        let mut store = store(40);
        let filtered = |start, direction, page_size| TransactionListInput {
            account_id: Some(account(2)),
            ..input(start, direction, page_size)
        };
        let all = account_two_keys(40);
        let first = page(&mut store, filtered(None, Direction::Forward, SCREEN_ROWS));
        assert_eq!(item_keys(&first), all[..10]);
        assert_eq!(first.next_key, Some(all[10]));
        let second = page(
            &mut store,
            filtered(first.next_key, Direction::Forward, SCREEN_ROWS),
        );
        assert_eq!(item_keys(&second), all[10..]);
        assert_eq!(
            second.message,
            Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE)
        );
        let back = page(
            &mut store,
            filtered(second.prev_key, Direction::Backward, SCREEN_ROWS),
        );
        assert_eq!(item_keys(&back), all[..10]);

        // The dashboard's latest five of account 2: from the end of the file.
        let latest = page(&mut store, filtered(None, Direction::Backward, 5));
        assert_eq!(item_keys(&latest), all[8..]);

        // A start key on another account's row starts at the account's next
        // row in both directions.
        let from_other = page(&mut store, filtered(Some(key(4)), Direction::Forward, 2));
        assert_eq!(item_keys(&from_other), [key(6), key(9)]);
        let before_other = page(&mut store, filtered(Some(key(4)), Direction::Backward, 2));
        assert_eq!(item_keys(&before_other), [key(3)]);
        assert_eq!(before_other.next_key, Some(key(6)));

        let unfiltered = page(&mut store, input(None, Direction::Forward, SCREEN_ROWS));
        assert_eq!(item_keys(&unfiltered), keys(1..=10));
        let mut other = FakeStore::default().with_xref(xref(CARD_A, 1));
        let nothing = page(
            &mut other,
            TransactionListInput {
                account_id: Some(account(9)),
                ..input(None, Direction::Forward, SCREEN_ROWS)
            },
        );
        assert_eq!(
            nothing,
            TransactionListPage::empty(messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE)
        );
    }

    #[test]
    fn a_failed_browse_is_the_lookup_error() {
        // P-COTRN00C-012 to P-COTRN00C-014, V-COTRN00C-006, V-COTRN00C-008,
        // V-COTRN00C-011, V-COTRN00C-013: RESP other than NORMAL, NOTFND or
        // ENDFILE sets WS-ERR-FLG.
        let expected = DomainError::unexpected(messages::UNABLE_TO_LOOKUP_TRANSACTION_COTRN00C)
            .with_field("TRNIDIN");
        assert_eq!(expected.kind, ErrorKind::Unexpected);
        for (start, direction) in [
            (None, Direction::Forward),
            (Some(key(5)), Direction::Forward),
            (Some(key(5)), Direction::Backward),
            (None, Direction::Backward),
        ] {
            let mut store = store(23);
            store.fail_next(
                "browse_transactions",
                PortError::Unavailable("connection reset".to_owned()),
            );
            assert_eq!(
                block_on(list(&mut store, input(start, direction, SCREEN_ROWS))),
                Err(expected.clone()),
                "{start:?} {direction:?}"
            );
        }
    }

    #[test]
    fn the_rule_future_is_send() {
        let mut store = store(2);
        let shown = block_on(require_send(list(
            &mut store,
            input(None, Direction::Forward, SCREEN_ROWS),
        )))
        .expect("a page");
        assert_eq!(item_keys(&shown), keys(1..=2));
    }
}
