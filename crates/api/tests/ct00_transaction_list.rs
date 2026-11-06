//! CT00 `GET /transactions` (`COTRN00C`, map `COTRN00`) through the HTTP
//! layer: the browse of the live transaction master in TRAN-ID order, its
//! page-boundary texts, the start-key edit, the failed browse and the
//! optional account filter, and the REST replacements of the map's AID keys
//! and of the transfer to sign-on without a session.
//!
//! Every status, BMS field and browse outcome is read from a reviewed
//! `V-COTRN00C` row of `docs/parity/validation-audit.md` or `P-` row of
//! `docs/parity/parity-oracle-audit.md`, and every text is a
//! `carddemo_domain::messages` constant that the test also finds, quoted, in
//! that row. A row whose review did not pass fails the test before it is used.
//!
//! The seeded master is empty (E-07), and a CT02 add against an empty master
//! allocates no TRAN-ID (E-27). Two live rows of account 3 are therefore
//! written first, then 23 transactions are added through CT02 itself: 15 for
//! account 1 by account ID and 8 for account 2 by card number, interleaved.
//! The 25 live TRAN-IDs fill pages of 10, 10 and 5 rows, and the gap between
//! the two first rows holds a start key that names no record.
//!
//! Parity is measured on the unfiltered browse: `account_id` is the REST
//! resource's added filter, which `COTRN00C` does not have (E-17).
//!
//! Docker must be running: the browse test starts one `postgres:16.15`
//! container; the harness conventions are those of `common`.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};

use carddemo_api::dto::transactions::{
    TransactionAddRequest, TransactionAddResponse, TransactionListField, TransactionResponse,
};
use carddemo_api::pagination::Page;
use carddemo_audit::{Document, cell_code_spans, code_span, is_no_message, key_values};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::block_on;
use reqwest::StatusCode;
use serde_json::{Value, json};

use common::{Resp, TestApp, assert_error, fixture_records};

/// The processing clock of the adds, inside the active period of the seeded
/// retail reward rule.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The origination and processing date of every add, in the `YYYY-MM-DD`
/// form `TORIGDT` and `TPROCDT` take.
const ADD_DATE: &str = "2022-07-01";

/// The CT00 list route, also the CT02 add route.
const TRANSACTIONS: &str = "/transactions";

/// Account 1 of the seed (P-CVACT01Y-015), added to by its account ID. The
/// card its adds answer with is decoded from its cross-reference.
const ACCOUNT_ONE: &str = "00000000001";

/// Account 2 of the seed, added to by its card number.
const ACCOUNT_TWO: &str = "00000000002";

/// The card number the account 2 adds enter as `CARDNIN`
/// [app/data/ASCII/cardxref.txt:L3]; its decoded cross-reference must give
/// account 2.
const ACCOUNT_TWO_CARD: &str = "0923877193247330";

/// The CT02 adds for account 1.
const ACCOUNT_ONE_ADDS: usize = 15;

/// The CT02 adds for account 2.
const ACCOUNT_TWO_ADDS: usize = 8;

/// The TRAN-IDs of the two live rows written before the adds.
const BASE_KEYS: [&str; 2] = ["0000000000000001", "0000000000000009"];

/// The two live rows the first CT02 allocation counts from, on the card of
/// account 3 [app/data/ASCII/cardxref.txt:L17], so neither filtered list
/// holds them.
const BASE_ROWS: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES \
     ('0000000000000001', '01', 1, 'POS TERM', 'CT00 base row 1', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '3999169246375885', '2022-06-01 00:00:00.000000', '2022-06-01 00:00:00.000000'), \
     ('0000000000000009', '01', 1, 'POS TERM', 'CT00 base row 9', 20.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '3999169246375885', '2022-06-02 00:00:00.000000', '2022-06-02 00:00:00.000000')";

/// A start key between the two base rows, naming no record.
const BETWEEN_KEY: &str = "0000000000000005";

/// Start keys the `IS NUMERIC` test of `TRNIDINI X(16)` refuses: letters, the
/// audit's example with a letter in its last byte, and 15 digits, which the
/// field holds padded with a space.
const NON_NUMERIC_KEYS: [&str; 3] = ["ABC", "000000000068358X", "000000000000001"];

/// An account filter that is not 11 digits, the example of its row.
const NON_NUMERIC_ACCOUNT: &str = "0000000000A";

/// The rows of the dashboard's latest transactions of one account, read
/// backward from the end of the file.
const DASHBOARD_ROWS: usize = 5;

/// Hides the live master from the browse, whose unfiltered reads use only
/// this view: the next browse fails on the server with SQLSTATE `42P01`.
const HIDE_MASTER: &str = "ALTER VIEW transaction_master RENAME TO transaction_master_offline";

/// Puts the live master back.
const RESTORE_MASTER: &str = "ALTER VIEW transaction_master_offline RENAME TO transaction_master";

/// `direction` values that name no key of `COTRN00`: an AID key the map does
/// not offer, the CLEAR key, and ENTER, which is the request without a
/// `direction`.
const OTHER_AID_KEYS: [&str; 3] = ["pf9", "clear", "enter"];

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The HTTP methods an `OpenAPI` path item may declare an operation under.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// One CT00 page as `GET /transactions` answers it.
type TransactionPage = Page<TransactionResponse>;

/// What one validation audit row records, besides its message.
struct Recorded {
    /// `Status`.
    status: StatusCode,
    /// `BMS field`: `TRNIDIN`, or `—` for none.
    field: String,
    /// The `kind=` of `Trigger condition`: an `ErrorKind` name or `none`.
    kind: String,
}

/// The two audits the expectations come from.
struct Audits {
    validation: Document,
    parity: Document,
}

impl Audits {
    fn load() -> Self {
        Self {
            validation: common::validation_audit(),
            parity: common::parity_audit(),
        }
    }

    /// The validation row `id`, once its review passed and its
    /// `Message text` is `message` quoted, or `no-message` when `message` is
    /// empty.
    fn validation(&self, id: &str, message: &str) -> Recorded {
        assert_reviewed(&self.validation, id);
        let (table, row) = self
            .validation
            .find_row(id)
            .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
        let cell = |name: &str| {
            table
                .cell(row, name)
                .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        };
        let text = cell("Message text");
        if message.is_empty() {
            assert!(is_no_message(text), "{id} records a message: {text}");
        } else {
            assert_eq!(unquoted(text).as_deref(), Some(message), "the text of {id}");
        }
        let status = cell("Status");
        let status = status
            .parse::<u16>()
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {status:?}"));
        let kind = key_values(cell("Trigger condition"))
            .into_iter()
            .rev()
            .find(|pair| pair.key == "kind")
            .unwrap_or_else(|| panic!("{id} records no kind"))
            .value;
        Recorded {
            status,
            field: cell("BMS field").to_owned(),
            kind,
        }
    }

    /// Checks that the validation row `id` records `message` as a
    /// page-boundary text: a 200 page that sets no error flag, with the cursor
    /// on `TRNIDIN`.
    fn page_text(&self, id: &str, message: &str) {
        let row = self.validation(id, message);
        assert_eq!(row.status, StatusCode::OK, "the status of {id}");
        assert_eq!(row.kind, "none", "the kind of {id}");
        assert_eq!(
            row.field,
            TransactionListField::TrnIdIn.as_str(),
            "the BMS field of {id}"
        );
    }

    /// Asserts that `resp` is the error answer the validation row `id`
    /// records: its status, the code of `kind`, and `message` on its BMS
    /// field, or no field when the row sets no message.
    fn assert_answer(&self, id: &str, kind: ErrorKind, message: &str, resp: &Resp) {
        let row = self.validation(id, message);
        assert_eq!(row.kind, format!("{kind:?}"), "the kind of {id}");
        let fields = if message.is_empty() {
            Vec::new()
        } else {
            assert_eq!(
                row.field,
                TransactionListField::TrnIdIn.as_str(),
                "the BMS field of {id}"
            );
            vec![(row.field.as_str(), message)]
        };
        assert_error(resp, row.status, kind.code(), message, &fields);
    }

    /// The `Expected result` of the parity row `id`, once its review passed
    /// and every text of `texts` is one of its quoted code spans.
    fn parity(&self, id: &str, texts: &[&str]) -> String {
        assert_reviewed(&self.parity, id);
        let (table, row) = self
            .parity
            .find_row(id)
            .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
        let result = table
            .cell(row, "Expected result")
            .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
        let spans: Vec<String> = cell_code_spans(result)
            .iter()
            .filter_map(|span| quoted_text(&span.text).map(str::to_owned))
            .collect();
        for text in texts {
            assert!(
                spans.iter().any(|span| span == text),
                "{id} records no {text:?}: {result}"
            );
        }
        result.to_owned()
    }

    /// The value of `key` in the `Expected result` of the parity row `id`.
    fn parity_value(&self, id: &str, key: &str) -> String {
        let result = self.parity(id, &[]);
        key_values(&result)
            .into_iter()
            .find(|pair| pair.key == key)
            .unwrap_or_else(|| panic!("{id} records no {key}: {result}"))
            .value
    }

    /// The quoted `value` that follows `kind=<kind>` in the section D row
    /// `id`, unquoted.
    fn fixture_value(&self, id: &str, kind: &str) -> String {
        let result = self.parity(id, &[]);
        let pairs = key_values(&result);
        pairs
            .iter()
            .skip_while(|pair| !(pair.key == "kind" && pair.value == kind))
            .skip(1)
            .take_while(|pair| pair.key != "kind")
            .find(|pair| pair.key == "value")
            .and_then(|pair| unquoted(&pair.value))
            .unwrap_or_else(|| panic!("{id} records no {kind} value: {result}"))
    }

    /// The rows of one COTRN00 page: the `page_size` of P-COTRN00C-006.
    fn screen_rows(&self) -> usize {
        let rows = self.parity_value("P-COTRN00C-006", "page_size");
        rows.parse()
            .unwrap_or_else(|_| panic!("P-COTRN00C-006 records no row count: {rows:?}"))
    }
}

/// Asserts that the audit `doc` holds a review of the row `id` and that every
/// review of it passed.
fn assert_reviewed(doc: &Document, id: &str) {
    let mut verdicts = Vec::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if table.cell(row, "Row ID") == Some(id) {
                verdicts.push(table.cell(row, "Verdict").unwrap_or_default());
            }
        }
    }
    assert!(
        !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == "pass"),
        "{id} must have passed its review, found {verdicts:?}"
    );
}

/// `text` without the double quotes the audits put around every exact text,
/// or `None` when it is not quoted.
fn quoted_text(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// The quoted text of the first code span of `cell`, unquoted.
fn unquoted(cell: &str) -> Option<String> {
    let span = code_span(cell)?;
    quoted_text(&span.text).map(str::to_owned)
}

/// The page a request must answer: its TRAN-IDs in order, its keys and its
/// page-boundary text.
struct Expected<'a> {
    keys: &'a [String],
    next_key: Option<&'a str>,
    prev_key: Option<&'a str>,
    message: Option<&'a str>,
}

/// Asserts that `shown`, named `what` in a failure, is the page `expected`.
fn assert_page(shown: &TransactionPage, expected: &Expected<'_>, what: &str) {
    let keys: Vec<&str> = shown
        .items
        .iter()
        .map(|item| item.tran_id.as_str())
        .collect();
    let wanted: Vec<&str> = expected.keys.iter().map(String::as_str).collect();
    assert_eq!(keys, wanted, "the TRAN-IDs of {what}");
    assert_eq!(
        shown.next_key.as_deref(),
        expected.next_key,
        "the next_key of {what}"
    );
    assert_eq!(
        shown.prev_key.as_deref(),
        expected.prev_key,
        "the prev_key of {what}"
    );
    assert_eq!(
        shown.message.as_deref(),
        expected.message,
        "the message of {what}"
    );
}

/// The TRAN-IDs of the rows of `shown`, in order.
fn tran_ids(shown: &TransactionPage) -> Vec<String> {
    shown
        .items
        .iter()
        .map(|item| item.tran_id.clone())
        .collect()
}

/// `GET /transactions` with the query parameters `params`, in order.
fn list_path(params: &[(&str, &str)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    if query.is_empty() {
        TRANSACTIONS.to_owned()
    } else {
        format!("{TRANSACTIONS}?{}", query.join("&"))
    }
}

/// The signed-on user's requests, and the audits their answers are checked
/// against.
struct Ct00<'a> {
    app: &'a TestApp,
    token: &'a str,
    audits: &'a Audits,
}

impl Ct00<'_> {
    /// The raw answer to `GET /transactions` with `params`.
    async fn get(&self, params: &[(&str, &str)]) -> Resp {
        self.app.get(&list_path(params), Some(self.token)).await
    }

    /// The 200 page `GET /transactions` answers with `params`.
    ///
    /// Each answer is one JSON page, `{items, next_key, prev_key, message?}`,
    /// and no member carries the 3270 header (P-COTRN00C-018,
    /// P-COTRN00C-020).
    async fn page(&self, params: &[(&str, &str)]) -> TransactionPage {
        let path = list_path(params);
        let (status, body, raw) = self.app.get(&path, Some(self.token)).await;
        assert_eq!(status, StatusCode::OK, "the status of GET {path}: {raw}");
        let required = BTreeSet::from(["items", "next_key", "prev_key"]);
        let members: BTreeSet<&str> = body
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert!(
            members.is_superset(&required)
                && members.difference(&required).all(|name| *name == "message"),
            "GET {path} answers {{items, next_key, prev_key, message?}}: {raw}"
        );
        serde_json::from_value(body)
            .unwrap_or_else(|error| panic!("GET {path} is no transaction page ({error}): {raw}"))
    }

    /// The pages from the first page of `filter` on, each read forward from
    /// the `next_key` of the page before, until a page has none.
    async fn forward_pages(&self, filter: &[(&str, &str)]) -> Vec<TransactionPage> {
        let mut pages = vec![self.page(filter).await];
        while let Some(next) = pages.last().and_then(|last| last.next_key.clone()) {
            assert!(pages.len() < 10, "the browse of {filter:?} never ended");
            let mut params = filter.to_vec();
            params.extend([("start_key", next.as_str()), ("direction", "forward")]);
            pages.push(self.page(&params).await);
        }
        pages
    }
}

/// The live master once loaded.
struct Loaded {
    /// Every live TRAN-ID, ascending.
    keys: Vec<String>,
    /// The TRAN-IDs added for account 1, ascending.
    account_one: Vec<String>,
    /// The TRAN-IDs added for account 2, ascending.
    account_two: Vec<String>,
    /// The record each CT02 add answered with, by TRAN-ID.
    added: BTreeMap<String, TransactionResponse>,
}

/// The key a CT02 add names.
#[derive(Debug, Clone, Copy)]
enum AddKey {
    /// `ACTIDIN`: the account ID.
    Account(&'static str),
    /// `CARDNIN`: the card number, the account ID left blank.
    Card(&'static str),
}

/// The `TRNAMT` of the add numbered `index`: a sign, eight digits, a point and
/// two digits, a different positive amount for each add.
fn amount(index: usize) -> String {
    format!("+{:08}.{:02}", 12 + index, index % 100)
}

/// Adds one confirmed purchase of `amount` through CT02 for the account or
/// card `key`, and returns the record the 201 answer carries.
async fn add_transaction(
    app: &TestApp,
    token: &str,
    key: AddKey,
    amount: &str,
) -> TransactionResponse {
    let (account_id, card_number) = match key {
        AddKey::Account(account) => (Some(account.to_owned()), None),
        AddKey::Card(card) => (None, Some(card.to_owned())),
    };
    let text = |value: &str| Some(value.to_owned());
    let request = TransactionAddRequest {
        account_id,
        card_number,
        tran_type_cd: text("01"),
        tran_cat_cd: text("0001"),
        tran_source: text("POS TERM"),
        tran_desc: text("CT00 browse purchase"),
        tran_amt: text(amount),
        tran_orig_date: text(ADD_DATE),
        tran_proc_date: text(ADD_DATE),
        tran_merchant_id: text("000000001"),
        tran_merchant_name: text("CT00 browse merchant"),
        tran_merchant_city: text("CT00 browse city"),
        tran_merchant_zip: text("12345"),
        confirm: text("Y"),
    };
    let body = serde_json::to_value(&request)
        .unwrap_or_else(|error| panic!("the CT02 body does not serialize: {error}"));
    let (status, json, raw) = app.post(TRANSACTIONS, Some(token), &body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the CT02 add for {key:?}: {raw}"
    );
    let added: TransactionAddResponse = serde_json::from_value(json)
        .unwrap_or_else(|error| panic!("the CT02 answer does not decode ({error}): {raw}"));
    assert_eq!(
        added.message,
        messages::transaction_added_message(&added.transaction.tran_id),
        "the CT02 success text: {raw}"
    );
    added.transaction
}

/// Writes the two base rows, then adds the 23 transactions through CT02,
/// every third add from the second on for account 2 by card number and the
/// others for account 1 by account ID.
///
/// Each add takes the highest live TRAN-ID plus one, so the adds follow the
/// higher base row with consecutive keys, and each answers with its success
/// text (P-COTRN02C-024).
///
/// An add by account ID writes the card of the account's cross-reference
/// (P-COTRN02C-023): the `xref_card_num` of the one `cardxref.txt` record
/// whose `xref_acct_id` is account 1, decoded at its reviewed section A
/// offsets (P-CVACT03Y-001, P-CVACT03Y-003). The card the account 2 adds
/// name is checked to cross-reference account 2 the same way, so the account
/// 2 filter holds exactly those adds.
async fn load_master(app: &TestApp, token: &str, audits: &Audits) -> Loaded {
    audits.parity("P-COTRN02C-024", &[]);
    audits.parity("P-COTRN02C-023", &[]);
    assert_eq!(
        audits.fixture_value("P-CVACT01Y-015", "existing_account"),
        ACCOUNT_ONE,
        "the seeded account the adds name"
    );
    let account_one_xref = fixture_records::account_xref(&audits.parity, ACCOUNT_ONE);
    let account_one_card = account_one_xref.value("xref_card_num");
    assert_eq!(
        fixture_records::card_xref(&audits.parity, ACCOUNT_TWO_CARD).value("xref_acct_id"),
        ACCOUNT_TWO,
        "P-CVACT03Y-001, P-CVACT03Y-003: the cross-reference of the card the account 2 adds \
         name gives account 2"
    );
    app.stack().exec_sql(BASE_ROWS).await;
    let mut loaded = Loaded {
        keys: BASE_KEYS.map(str::to_owned).to_vec(),
        account_one: Vec::new(),
        account_two: Vec::new(),
        added: BTreeMap::new(),
    };
    let total = ACCOUNT_ONE_ADDS + ACCOUNT_TWO_ADDS;
    for index in 0..total {
        let for_two = index % 3 == 1;
        let (key, card) = if for_two {
            (AddKey::Card(ACCOUNT_TWO_CARD), ACCOUNT_TWO_CARD)
        } else {
            (AddKey::Account(ACCOUNT_ONE), account_one_card)
        };
        let added = add_transaction(app, token, key, &amount(index)).await;
        assert_eq!(
            added.tran_card_num, card,
            "P-COTRN02C-023: the card of the add for {key:?} is the card of its cross-reference"
        );
        let tran_id = added.tran_id.clone();
        if for_two {
            loaded.account_two.push(tran_id.clone());
        } else {
            loaded.account_one.push(tran_id.clone());
        }
        loaded.keys.push(tran_id.clone());
        loaded.added.insert(tran_id, added);
    }
    assert_eq!(loaded.account_one.len(), ACCOUNT_ONE_ADDS, "account 1 adds");
    assert_eq!(loaded.account_two.len(), ACCOUNT_TWO_ADDS, "account 2 adds");
    let highest_base = BASE_KEYS[BASE_KEYS.len() - 1];
    let first = highest_base
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("the base TRAN-ID {highest_base} is no number: {error}"))
        + 1;
    let allocated: Vec<String> = (first..).take(total).map(|n| format!("{n:016}")).collect();
    assert_eq!(
        loaded.keys[BASE_KEYS.len()..],
        allocated[..],
        "the TRAN-IDs CT02 allocated"
    );
    loaded
}

/// Before any write the seeded master is empty (E-07): forward and backward,
/// the STARTBR `NOTFND` page, without rows or keys, with its text
/// (V-COTRN00C-005, P-COTRN00C-011, E-27).
async fn empty_master(ct00: &Ct00<'_>) {
    let audits = ct00.audits;
    let rows = audits.parity_value("P-CVTRA05Y-016", "rows");
    assert!(
        rows.starts_with("0 "),
        "P-CVTRA05Y-016 records a seeded transaction master of {rows}"
    );
    let top = messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE;
    audits.page_text("V-COTRN00C-005", top);
    audits.parity("P-COTRN00C-011", &[top]);
    let empty = Expected {
        keys: &[],
        next_key: None,
        prev_key: None,
        message: Some(top),
    };
    for params in [&[][..], &[("direction", "backward")][..]] {
        let shown = ct00.page(params).await;
        assert_page(
            &shown,
            &empty,
            &format!("the empty master read with {params:?}"),
        );
    }
}

/// The unfiltered browse forward (P-COTRN00C-006): pages of `rows` in
/// ascending TRAN-ID order, each `next_key` the first row of the next page,
/// the last page holding only the records read (P-COTRN00C-016) and the
/// end-of-file text (V-COTRN00C-007, P-COTRN00C-013); every row is the whole
/// record its add wrote (P-COTRN00C-008).
async fn unfiltered_browse(ct00: &Ct00<'_>, loaded: &Loaded, rows: usize) -> Vec<TransactionPage> {
    let audits = ct00.audits;
    let bottom = messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE;
    audits.page_text("V-COTRN00C-007", bottom);
    audits.parity("P-COTRN00C-013", &[bottom]);
    for id in ["P-COTRN00C-008", "P-COTRN00C-016"] {
        audits.parity(id, &[]);
    }
    let pages = ct00.forward_pages(&[]).await;
    let chunks: Vec<&[String]> = loaded.keys.chunks(rows).collect();
    assert_eq!(pages.len(), chunks.len(), "the unfiltered pages");
    for (number, (shown, keys)) in pages.iter().zip(&chunks).enumerate() {
        let next_key = loaded.keys.get((number + 1) * rows).map(String::as_str);
        let expected = Expected {
            keys,
            next_key,
            // ENTER from the lowest key offers no previous page.
            prev_key: keys.first().map(String::as_str).filter(|_| number > 0),
            message: next_key.is_none().then_some(bottom),
        };
        assert_page(shown, &expected, &format!("unfiltered page {}", number + 1));
    }
    let shown: Vec<String> = pages.iter().flat_map(tran_ids).collect();
    assert_eq!(
        shown, loaded.keys,
        "the pages concatenate to the live master"
    );
    for item in pages.iter().flat_map(|shown| &shown.items) {
        if let Some(added) = loaded.added.get(&item.tran_id) {
            assert_eq!(item, added, "the listed record of {}", item.tran_id);
        }
    }
    pages
}

/// PF7 from the first key of each later page shows the page before it
/// (P-COTRN00C-007), the first one with the start-of-file text
/// (V-COTRN00C-012, P-COTRN00C-014); PF7 on the first page reads nothing
/// (V-COTRN00C-009, P-COTRN00C-009).
async fn backward_browse(ct00: &Ct00<'_>, loaded: &Loaded, pages: &[TransactionPage]) {
    let audits = ct00.audits;
    let reached = messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE;
    let already = messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE;
    audits.parity("P-COTRN00C-007", &[]);
    audits.page_text("V-COTRN00C-012", reached);
    audits.parity("P-COTRN00C-014", &[reached]);
    audits.page_text("V-COTRN00C-009", already);
    audits.parity("P-COTRN00C-009", &[already]);
    for (earlier, later) in pages.iter().zip(pages.iter().skip(1)) {
        let start = later
            .prev_key
            .as_deref()
            .unwrap_or_else(|| panic!("a later page offers no prev_key: {later:?}"));
        let shown = ct00
            .page(&[("start_key", start), ("direction", "backward")])
            .await;
        let expected = Expected {
            keys: &tran_ids(earlier),
            next_key: Some(start),
            prev_key: earlier.prev_key.as_deref(),
            message: earlier.prev_key.is_none().then_some(reached),
        };
        assert_page(&shown, &expected, &format!("PF7 from {start}"));
    }
    let lowest = loaded.keys.first().map_or("", String::as_str);
    let shown = ct00
        .page(&[("start_key", lowest), ("direction", "backward")])
        .await;
    let expected = Expected {
        keys: &[],
        next_key: Some(lowest),
        prev_key: None,
        message: Some(already),
    };
    assert_page(&shown, &expected, "PF7 on the first page");
}

/// The start key: `STARTBR` positions at the first TRAN-ID at or after it
/// (P-COTRN00C-006); from the last TRAN-ID it is the PF8 without a next page
/// (V-COTRN00C-003, P-COTRN00C-010); above the last one it finds nothing in
/// either direction (V-COTRN00C-005, V-COTRN00C-010); and a key that is not
/// 16 digits is refused before any read (V-COTRN00C-004, P-COTRN00C-005).
async fn start_keys(ct00: &Ct00<'_>, loaded: &Loaded, rows: usize) {
    let audits = ct00.audits;
    let keys = &loaded.keys;
    let shown = ct00.page(&[("start_key", BETWEEN_KEY)]).await;
    let expected = Expected {
        keys: &keys[1..=rows],
        next_key: keys.get(rows + 1).map(String::as_str),
        prev_key: keys.get(1).map(String::as_str),
        message: None,
    };
    assert_page(&shown, &expected, &format!("ENTER with {BETWEEN_KEY}"));

    let bottom = messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE;
    audits.page_text("V-COTRN00C-003", bottom);
    audits.parity("P-COTRN00C-010", &[bottom]);
    let highest = &keys[keys.len() - 1..];
    let last = highest.first().map_or("", String::as_str);
    let shown = ct00
        .page(&[("start_key", last), ("direction", "forward")])
        .await;
    let expected = Expected {
        keys: highest,
        next_key: None,
        prev_key: Some(last),
        message: Some(bottom),
    };
    assert_page(&shown, &expected, "PF8 from the last TRAN-ID");

    let top = messages::YOU_ARE_AT_THE_TOP_OF_THE_PAGE;
    let missing = audits.fixture_value("P-CVTRA05Y-016", "missing_tran");
    for (id, direction) in [
        ("V-COTRN00C-005", "forward"),
        ("V-COTRN00C-010", "backward"),
    ] {
        audits.page_text(id, top);
        let shown = ct00
            .page(&[("start_key", &missing), ("direction", direction)])
            .await;
        let expected = Expected {
            keys: &[],
            next_key: None,
            prev_key: None,
            message: Some(top),
        };
        assert_page(&shown, &expected, &format!("{direction} from {missing}"));
    }

    let numeric = messages::TRAN_ID_MUST_BE_NUMERIC;
    audits.parity("P-COTRN00C-005", &[numeric]);
    for key in NON_NUMERIC_KEYS {
        let refused = ct00.get(&[("start_key", key)]).await;
        audits.assert_answer("V-COTRN00C-004", ErrorKind::Invalid, numeric, &refused);
    }
}

/// The CT02 "copy last transaction" source, a backward page of one row
/// without a start key: the highest-keyed transaction of the whole file,
/// whatever its account (P-COTRN02C-026). Then the `S` selector's
/// replacement, the row's view link: `GET /transactions/{id}` answers the
/// record the list shows (P-COTRN00C-003).
async fn copy_last_and_view(ct00: &Ct00<'_>, loaded: &Loaded, pages: &[TransactionPage]) {
    let audits = ct00.audits;
    audits.parity("P-COTRN02C-026", &[]);
    let highest = &loaded.keys[loaded.keys.len() - 1..];
    let last = highest.first().map_or("", String::as_str);
    let shown = ct00
        .page(&[("direction", "backward"), ("page_size", "1")])
        .await;
    let expected = Expected {
        keys: highest,
        next_key: None,
        prev_key: Some(last),
        message: None,
    };
    assert_page(&shown, &expected, "the copy-last-transaction source");
    assert_eq!(
        shown.items.first(),
        loaded.added.get(last),
        "the copy-last-transaction source is the last add"
    );

    audits.parity("P-COTRN00C-003", &[]);
    for item in pages.iter().filter_map(|shown| shown.items.first()) {
        let path = format!("{TRANSACTIONS}/{}", item.tran_id);
        let (status, body, raw) = ct00.app.get(&path, Some(ct00.token)).await;
        assert_eq!(status, StatusCode::OK, "the status of GET {path}: {raw}");
        let viewed: TransactionResponse = serde_json::from_value(body)
            .unwrap_or_else(|error| panic!("GET {path} is no transaction ({error}): {raw}"));
        assert_eq!(&viewed, item, "GET {path} shows the listed record");
    }
}

/// The `account_id` filter the REST resource adds (E-17, P-COTRN00C-015):
/// the same ordering and paging over the account's transactions only, the
/// dashboard's latest rows among them; a filter that is not 11 digits sets no
/// message (V-COTRN00C-001).
async fn account_filter(ct00: &Ct00<'_>, loaded: &Loaded, rows: usize) {
    let audits = ct00.audits;
    audits.parity("P-COTRN00C-015", &[]);
    let bottom = messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE;
    let one = &loaded.account_one;
    let pages = ct00.forward_pages(&[("account_id", ACCOUNT_ONE)]).await;
    let [first, second] = pages.as_slice() else {
        panic!("account 1 must list on two pages: {pages:?}");
    };
    let second_start = one.get(rows).map(String::as_str);
    let expected = Expected {
        keys: &one[..rows],
        next_key: second_start,
        prev_key: None,
        message: None,
    };
    assert_page(first, &expected, "account 1, page 1");
    let expected = Expected {
        keys: &one[rows..],
        next_key: None,
        prev_key: second_start,
        message: Some(bottom),
    };
    assert_page(second, &expected, "account 1, page 2");

    let start = second_start.unwrap_or_default();
    let back = ct00
        .page(&[
            ("account_id", ACCOUNT_ONE),
            ("start_key", start),
            ("direction", "backward"),
        ])
        .await;
    let expected = Expected {
        keys: &one[..rows],
        next_key: Some(start),
        prev_key: None,
        message: Some(messages::YOU_HAVE_REACHED_THE_TOP_OF_THE_PAGE),
    };
    assert_page(&back, &expected, "account 1, PF7 from page 2");

    let size = DASHBOARD_ROWS.to_string();
    let latest = ct00
        .page(&[
            ("account_id", ACCOUNT_ONE),
            ("direction", "backward"),
            ("page_size", &size),
        ])
        .await;
    let tail = &one[one.len() - DASHBOARD_ROWS..];
    let expected = Expected {
        keys: tail,
        next_key: None,
        prev_key: tail.first().map(String::as_str),
        message: None,
    };
    assert_page(&latest, &expected, "the latest rows of account 1");

    let two = ct00.forward_pages(&[("account_id", ACCOUNT_TWO)]).await;
    let [only] = two.as_slice() else {
        panic!("account 2 must list on one page: {two:?}");
    };
    let expected = Expected {
        keys: &loaded.account_two,
        next_key: None,
        prev_key: None,
        message: Some(bottom),
    };
    assert_page(only, &expected, "account 2");

    let refused = ct00.get(&[("account_id", NON_NUMERIC_ACCOUNT)]).await;
    audits.assert_answer("V-COTRN00C-001", ErrorKind::Invalid, "", &refused);
}

/// A failed browse sets `WS-ERR-FLG`: 500 with the lookup text on `TRNIDIN`,
/// forward (V-COTRN00C-006, V-COTRN00C-008, P-COTRN00C-012, P-COTRN00C-013)
/// and backward (V-COTRN00C-011, V-COTRN00C-013, P-COTRN00C-014). The one SQL
/// browse of a page stands for `STARTBR` and its reads alike, so each request
/// gives the answer both rows of its direction record. The master is put back
/// and lists as before.
async fn failed_browse(ct00: &Ct00<'_>, loaded: &Loaded, rows: usize) {
    let audits = ct00.audits;
    let lookup = messages::UNABLE_TO_LOOKUP_TRANSACTION_COTRN00C;
    for id in ["P-COTRN00C-012", "P-COTRN00C-013", "P-COTRN00C-014"] {
        audits.parity(id, &[lookup]);
    }
    let before = ct00.page(&[]).await;
    let start = loaded.keys.get(rows).map_or("", String::as_str);
    ct00.app.stack().exec_sql(HIDE_MASTER).await;
    let forward = ct00.get(&[]).await;
    let backward = ct00
        .get(&[("start_key", start), ("direction", "backward")])
        .await;
    ct00.app.stack().exec_sql(RESTORE_MASTER).await;
    for id in ["V-COTRN00C-006", "V-COTRN00C-008"] {
        audits.assert_answer(id, ErrorKind::Unexpected, lookup, &forward);
    }
    for id in ["V-COTRN00C-011", "V-COTRN00C-013"] {
        audits.assert_answer(id, ErrorKind::Unexpected, lookup, &backward);
    }
    assert_eq!(
        ct00.page(&[]).await,
        before,
        "the browse once the master is back"
    );
}

/// The AID keys `MAIN-PARA` dispatches on, as the list request carries them
/// (P-COTRN00C-001; F-COTRN00-001, F-COTRN00-003, F-COTRN00-004): ENTER is the
/// request without a `direction`, which reads forward from its start key
/// exactly as `direction=forward` does, the PF8 from the `next_key` shown; PF7
/// is `direction=backward` from the first key shown. Any other key is refused
/// before a page is read, and never with the invalid-key text, which has no
/// web control (F-COTRN00-006, E-56).
async fn aid_keys(ct00: &Ct00<'_>, pages: &[TransactionPage]) {
    let invalid_key = messages::CCDA_MSG_INVALID_KEY;
    ct00.audits.parity("P-COTRN00C-001", &[invalid_key]);
    for shown in pages {
        let Some(first) = shown.items.first() else {
            panic!("a listed page holds no row: {shown:?}");
        };
        let start = first.tran_id.as_str();
        let enter = ct00.page(&[("start_key", start)]).await;
        let pf8 = ct00
            .page(&[("start_key", start), ("direction", "forward")])
            .await;
        assert_eq!(enter, pf8, "ENTER and PF8 from {start}");
        assert_eq!(tran_ids(&enter), tran_ids(shown), "ENTER from {start}");
    }
    for (earlier, later) in pages.iter().zip(pages.iter().skip(1)) {
        let first = later
            .prev_key
            .as_deref()
            .unwrap_or_else(|| panic!("a later page offers no prev_key: {later:?}"));
        let pf7 = ct00
            .page(&[("start_key", first), ("direction", "backward")])
            .await;
        assert_eq!(tran_ids(&pf7), tran_ids(earlier), "PF7 from {first}");
    }
    for key in OTHER_AID_KEYS {
        let (status, body, raw) = ct00.get(&[("direction", key)]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "direction={key}: {raw}");
        assert_eq!(
            body["code"].as_str(),
            Some(ErrorKind::Invalid.code()),
            "direction={key}: {raw}"
        );
        assert_ne!(
            body["message"].as_str(),
            Some(invalid_key),
            "direction={key} answers the invalid-key text: {raw}"
        );
        assert!(body.get("items").is_none(), "direction={key} lists: {raw}");
    }
}

/// `EIBCALEN = 0` and PF3 transfer control without any file access
/// (P-COTRN00C-017): the request without a session is answered 401 before the
/// browse, so it stays 401 while every browse of a signed-on user fails, and
/// PF3 is the Back link, which sends no request (F-COTRN00-002).
async fn signed_off_browse(ct00: &Ct00<'_>, loaded: &Loaded) {
    let result = ct00.audits.parity("P-COTRN00C-017", &[]);
    for recorded in ["no file access", "no token gives 401"] {
        assert!(result.contains(recorded), "P-COTRN00C-017: {result}");
    }
    let start = loaded.keys.first().map_or("", String::as_str);
    let backward = list_path(&[("start_key", start), ("direction", "backward")]);
    ct00.app.stack().exec_sql(HIDE_MASTER).await;
    let signed_on = ct00.get(&[]).await;
    let mut signed_off = Vec::new();
    for path in [TRANSACTIONS, backward.as_str()] {
        signed_off.push((path, ct00.app.get(path, None).await));
    }
    ct00.app.stack().exec_sql(RESTORE_MASTER).await;
    assert_eq!(
        signed_on.0,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the hidden master fails a signed-on browse: {}",
        signed_on.2
    );
    for (path, resp) in &signed_off {
        assert_eq!(resp.0, StatusCode::UNAUTHORIZED, "GET {path}: {}", resp.2);
        assert_error(
            resp,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    }
}

/// The CT00 browse of one seeded database, from the empty master through the
/// loaded one; the failed and signed-off browses run last, as they hide the
/// master for a moment.
#[test]
fn ct00_browses_the_live_master_in_tran_id_order() {
    let audits = Audits::load();
    let rows = audits.screen_rows();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        let ct00 = Ct00 {
            app: &app,
            token: &token,
            audits: &audits,
        };
        empty_master(&ct00).await;
        let loaded = load_master(&app, &token, &audits).await;
        let pages = unfiltered_browse(&ct00, &loaded, rows).await;
        backward_browse(&ct00, &loaded, &pages).await;
        start_keys(&ct00, &loaded, rows).await;
        copy_last_and_view(&ct00, &loaded, &pages).await;
        account_filter(&ct00, &loaded, rows).await;
        aid_keys(&ct00, &pages).await;
        failed_browse(&ct00, &loaded, rows).await;
        signed_off_browse(&ct00, &loaded).await;
    });
}

/// The row selectors `SEL0001` to `SEL0010` have no REST parameter: the row's
/// view link replaces them, so their texts cannot be reached (E-54). The list
/// declares exactly its account filter and the shared page parameters, the
/// replacements of the start key and the PF7 and PF8 keys (V-COTRN00C-002,
/// P-COTRN00C-002, P-COTRN00C-004, P-COTRN00C-019).
#[test]
fn ct00_declares_no_row_selector_parameter() {
    let audits = Audits::load();
    let selection = messages::INVALID_SELECTION_VALID_VALUE_IS_S;
    audits.page_text("V-COTRN00C-002", selection);
    audits.parity("P-COTRN00C-002", &[]);
    audits.parity("P-COTRN00C-004", &[selection]);
    audits.parity("P-COTRN00C-019", &[]);
    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the API document is no JSON: {error}"));
    let parameters = &spec["paths"][TRANSACTIONS]["get"]["parameters"];
    let names: BTreeSet<&str> = parameters
        .as_array()
        .unwrap_or_else(|| panic!("GET {TRANSACTIONS} declares no parameters: {parameters}"))
        .iter()
        .filter_map(|parameter| parameter["name"].as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["account_id", "direction", "page_size", "start_key"]),
        "the query parameters of GET {TRANSACTIONS}"
    );
}

/// The operations the API document declares on every path of the transaction
/// routes, by path.
fn transaction_operations(spec: &Value) -> BTreeMap<String, BTreeSet<&'static str>> {
    let paths = spec["paths"]
        .as_object()
        .unwrap_or_else(|| panic!("the API document declares no paths"));
    paths
        .iter()
        .filter(|(path, _)| {
            path.as_str() == TRANSACTIONS || path.starts_with(&format!("{TRANSACTIONS}/"))
        })
        .map(|(path, item)| {
            let operations = HTTP_METHODS
                .into_iter()
                .filter(|method| item.get(*method).is_some())
                .collect();
            (path.clone(), operations)
        })
        .collect()
}

/// No key of `COTRN00` but ENTER, PF7 and PF8 reaches the API
/// (P-COTRN00C-001): the list's `direction` names exactly `forward` (ENTER and
/// PF8) and `backward` (PF7), so the invalid-key path has no request. PF3, the
/// transfer to the menu, is the Back link to `/`, navigation that sends no
/// request (P-COTRN00C-017, F-COTRN00-002): the transaction routes serve the
/// CT00 list, the CT02 add and the CT01 view, and nothing else.
#[test]
fn ct00_serves_enter_pf7_and_pf8_and_no_other_key() {
    let audits = Audits::load();
    audits.parity("P-COTRN00C-001", &[messages::CCDA_MSG_INVALID_KEY]);
    let replacement = audits.parity_value("P-COTRN00C-017", "replacement");
    assert!(
        replacement.contains("Back link to /") && replacement.contains("no request"),
        "P-COTRN00C-017 replaces PF3 with: {replacement}"
    );
    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the API document is no JSON: {error}"));
    let parameters = &spec["paths"][TRANSACTIONS]["get"]["parameters"];
    let direction = parameters
        .as_array()
        .and_then(|declared| {
            declared
                .iter()
                .find(|parameter| parameter["name"] == "direction")
        })
        .unwrap_or_else(|| panic!("GET {TRANSACTIONS} declares no direction: {parameters}"));
    assert_eq!(
        direction["schema"]["enum"],
        json!(["forward", "backward"]),
        "the keys `direction` names: {direction}"
    );
    let expected = BTreeMap::from([
        (TRANSACTIONS.to_owned(), BTreeSet::from(["get", "post"])),
        (format!("{TRANSACTIONS}/{{id}}"), BTreeSet::from(["get"])),
    ]);
    assert_eq!(
        transaction_operations(&spec),
        expected,
        "the operations of the transaction routes"
    );
}
