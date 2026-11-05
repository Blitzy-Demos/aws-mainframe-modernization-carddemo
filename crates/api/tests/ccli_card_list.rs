//! `CCLI` (`COCRDLIC`, map `COCRDLI`) through the HTTP layer: the card list
//! `GET /cards`, its browse in both directions, its two filters and the texts
//! its message line shows.
//!
//! Every expectation is read from the repository rather than restated here:
//!
//! - the cards and their key order come from `app/data/ASCII/carddata.txt`,
//!   every record decoded at the reviewed section A offsets of `CVACT02Y` and
//!   counted against section D, through `common::fixture_records`, once they
//!   agree with the fixture rows of `docs/parity/parity-oracle-audit.md`
//!   section D;
//! - the rows of a screen come from the `COCRDLIC` browse row of section B;
//! - every status, error `kind`, BMS field, `Order` and example input comes
//!   from the `COCRDLIC` rows of `docs/parity/validation-audit.md`, and every
//!   literal text from `carddemo_domain::messages` once it equals the row's
//!   quoted text;
//! - the read-failure text of V-COCRDLIC-008 and V-COCRDLIC-010 is the
//!   row's reviewed template, expanded through `common::audit_text` with the
//!   injected `READ`, the `ERROR-FILE` the row names and the E-44 codes, and
//!   cut at `WS-ERROR-MSG`, never composed by `carddemo_domain::messages`.
//!
//! The page state the COBOL keeps in its COMMAREA travels in the page keys
//! (E-54): `next_key` is the card the look-ahead `READNEXT` found after a full
//! page, and `prev_key` the first card shown. Because PF7 stays live on the
//! first screen, the first page carries its first card as `prev_key` too, and
//! the backward request from it answers 'NO PREVIOUS PAGES TO DISPLAY'.
//!
//! One seeded `postgres:16.15` container serves the whole file, so Docker must
//! be running. The input-error branch and the read-failure cases run last:
//! they hide the card table and put it back.

pub mod common;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::slice;

use carddemo_api::pagination::MAX_PAGE_SIZE;
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, KeyValue, Repo, classify_id, code_span, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};

use common::audit_text::{Operand, Template};
use common::fixture_records::{self, Record};
use common::{Resp, TestApp, assert_error};

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COCRDLIC";

/// The `CCLI` list route.
const CARDS: &str = "/cards";

/// The fixture row of the seeded card master: `table=` the table the seed
/// loads `carddata.txt` into, and `rows=` its record count.
const CARD_COUNT_ROW: &str = "P-CVACT02Y-008";

/// The table [`CARD_COUNT_ROW`] must name: the card master `CARDDAT`.
const CARD_TABLE: &str = "cards";

/// The `COCRDLIC` browse row: `page_size=` is `WS-MAX-SCREEN-LINES`, and the
/// cards come in ascending `CARD-NUM` order.
const BROWSE_ROW: &str = "P-COCRDLIC-003";

/// The fixture row of the unfiltered first page: its card numbers, accounts
/// and active statuses, rows 1 to 7.
const FIRST_PAGE_ROW: &str = "P-CVACT02Y-012";

/// The fixture row of card `0500024453765740`, the one card of account
/// `00000000050`, which the examples of V-COCRDLIC-005 and V-COCRDLIC-009 use.
const ACCOUNT_CARD_ROW: &str = "P-CVACT02Y-009";

/// The fixture row of account `99999999999`, which has no card.
const MISSING_ACCOUNT_ROW: &str = "P-CVACT01Y-016";

/// Account 1, whose one card is `9680294154603697`
/// [app/data/ASCII/carddata.txt:L49].
const ACCOUNT_ONE: &str = "00000000001";

/// A forward `start_key` above every card number: sixteen nines, the highest
/// key of `CARD-NUM` digits. PF8 after the last page was shown (E-54).
const PAST_LAST_CARD: &str = "9999999999999999";

/// The query parameters `GET /cards` publishes: the `ACCTSIDI` and
/// `CARDSIDI` filters and the shared pagination. No row selector `CRDSEL1` to
/// `CRDSEL7` is among them (E-54).
const LIST_PARAMETERS: [&str; 5] = [
    "account_id",
    "card_number",
    "start_key",
    "direction",
    "page_size",
];

/// The query parameter a 3270 AID key is sent in, named after `EIBAID`,
/// which `GET /cards` does not declare: no AID key reaches the API (E-56).
const AID_PARAMETER: &str = "EIBAID";

/// The AID keys `YYYY-STORE-PFKEY` stores under their own name besides the
/// PF keys [app/cpy/CSSTRPFY.cpy:L22-29].
const NAMED_AID_KEYS: [&str; 4] = ["ENTER", "CLEAR", "PA1", "PA2"];

/// PF1 to PF24, the PF keys `YYYY-STORE-PFKEY` stores, PF13 to PF24 as PF1
/// to PF12 [app/cpy/CSSTRPFY.cpy:L30-77].
const PF_KEY_COUNT: u8 = 24;

/// The function map row of PF3 on the `COCRDLI` map.
const PF3_CONTROL: &str = "F-COCRDLI-002";

/// The `Endpoint` cell of a function map row that is navigation only.
const NAVIGATION: &str = "— (navigation)";

/// The route of the dashboard, which holds the `COMEN01C` menu PF3 returns
/// to.
const MENU_ROUTE: &str = "/";

/// The function map rows of selectors `S` and `U` on the `COCRDLI` map,
/// which become the row actions View and Edit (E-54).
const ROW_ACTIONS: [&str; 2] = ["F-COCRDLI-005", "F-COCRDLI-006"];

/// The SPA route of one card, which every row action's route starts with.
const CARD_ROUTE: &str = "/cards/:cardNumber";

/// The `OpenAPI` schema of the `PUT /cards/{id}` body the edit route sends.
const UPDATE_SCHEMA: &str = "CardUpdateRequest";

/// The response key, and the `PUT /cards/{id}` body key, of a card's version
/// token.
const VERSION_KEY: &str = "version";

/// The hex digits of a version token.
const VERSION_LEN: usize = 16;

/// The keys a list row shares with the card it opens.
const ROW_KEYS: [&str; 3] = ["card_number", "account_id", "card_active_status"];

/// The paths of the card transactions in the `OpenAPI` document: the list
/// and one card.
const CARD_PATHS: [&str; 2] = [CARDS, "/cards/{id}"];

/// The HTTP methods an `OpenAPI` path item may declare.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The media type of every answer of the API.
const JSON: &str = "application/json";

/// The media type of a CICS `SEND TEXT`, which a request may ask for.
const PLAIN_TEXT: &str = "text/plain";

/// The parity row of the row-1 marker `1250-SETUP-ARRAY-ATTRIBS` never
/// shows; its `disposition=` quotes the marker.
const MARKER_ROW: &str = "P-COCRDLIC-030";

/// The row-1 selector of the `COCRDLI` map as a query parameter the API does
/// not read.
const ROW_ONE_SELECTOR: &str = "CRDSEL1";

/// Row-1 selectors, percent-encoded: empty, a space, `LOW-VALUES` and the
/// marker itself.
const ROW_ONE_SELECTORS: [&str; 4] = ["", "%20", "%00", "%2A"];

/// The V-COCRDLIC-003 example as query parameters the API does not read:
/// selector `S` on rows 1 and 2 of the unfiltered first page.
const TWO_SELECTED: &str = "CRDSEL1=S&CRDSEL2=S";

/// The V-COCRDLIC-004 example as a query parameter the API does not read:
/// selector `X` on row 1.
const BAD_ACTION: &str = "CRDSEL1=X";

/// Account filters that are not 11 digits besides the audit's example: not a
/// number, and ten digits, which leave `CC-ACCT-ID X(11)` not numeric.
const NOT_ACCOUNT_NUMBERS: [&str; 2] = ["ABC", "0000000001"];

/// A card filter of three digits, which leaves `CC-CARD-NUM X(16)` not
/// numeric.
const SHORT_CARD_NUMBER: &str = "123";

/// `ACCTSIDI PIC X(11)` [app/cpy-bms/COCRDLI.CPY:L66]: the width of the
/// account filter.
const ACCOUNT_FILTER_WIDTH: usize = 11;

/// `CARDSIDI PIC X(16)` [app/cpy-bms/COCRDLI.CPY:L72]: the width of the card
/// filter.
const CARD_FILTER_WIDTH: usize = 16;

/// The `message` of a filter longer than its map field: an input the 3270
/// field cannot hold has no COBOL text, so the answer carries none.
const NO_MESSAGE: &str = "";

/// The `kind=` of a validation row that sets no error flag: a 200 page.
const NO_KIND: &str = "none";

/// Every [`ErrorKind`], to read the `kind=` of a validation row by name.
const KINDS: [ErrorKind; 7] = [
    ErrorKind::Invalid,
    ErrorKind::Empty,
    ErrorKind::NotFound,
    ErrorKind::Duplicate,
    ErrorKind::Conflict,
    ErrorKind::SignonFailed,
    ErrorKind::Unexpected,
];

/// Hides the card master, so every card read fails on the server with
/// SQLSTATE `42P01`.
const HIDE_CARDS: &str = "ALTER TABLE cards RENAME TO cards_offline";

/// Puts the card master back.
const RESTORE_CARDS: &str = "ALTER TABLE cards_offline RENAME TO cards";

/// `MOVE 'READ' TO ERROR-OPNAME` of the `WHEN OTHER` branches of
/// `9000-READ-FORWARD` and `9100-READ-BACKWARDS`
/// [app/cbl/COCRDLIC.cbl:L1226] [app/cbl/COCRDLIC.cbl:L1312].
const ERROR_OPNAME_READ: &str = "READ";

/// `ERROR-OPNAME PIC X(8)` of `WS-FILE-ERROR-MESSAGE`
/// [app/cbl/COCRDLIC.cbl:L156].
const ERROR_OPNAME_WIDTH: usize = 8;

/// `ERROR-FILE PIC X(9)` of `WS-FILE-ERROR-MESSAGE`
/// [app/cbl/COCRDLIC.cbl:L160], which receives the 8-byte `LIT-CARD-FILE`.
const ERROR_FILE_WIDTH: usize = 9;

/// The digits of `WS-RESP-CD` and `WS-REAS-CD PIC S9(09) COMP`
/// [app/cbl/COCRDLIC.cbl:L47] [app/cbl/COCRDLIC.cbl:L49], whose unsigned
/// display a `MOVE` to an alphanumeric field gives.
const RESP_DIGITS: usize = 9;

/// `ERROR-RESP PIC X(10)` and `ERROR-RESP2 PIC X(10)` of
/// `WS-FILE-ERROR-MESSAGE` [app/cbl/COCRDLIC.cbl:L165]
/// [app/cbl/COCRDLIC.cbl:L169].
const ERROR_RESP_WIDTH: usize = 10;

/// `WS-ERROR-MSG PIC X(75)` [app/cbl/COCRDLIC.cbl:L117], the receiver
/// `WS-FILE-ERROR-MESSAGE` [app/cbl/COCRDLIC.cbl:L153-171] is moved to.
const ERROR_MSG_WIDTH: usize = 75;

/// The `RESP` E-44 sets for a database failure: `IOERR`.
const RESP_IOERR: u64 = 17;

/// The `RESP2` E-44 sets alongside [`RESP_IOERR`].
const RESP2_IOERR: u64 = 0;

/// The text an audit row expects.
#[derive(Debug, Clone, Copy)]
enum Text {
    /// A literal the catalog holds: the row's quoted text must equal it.
    Literal(&'static str),
    /// `WS-FILE-ERROR-MESSAGE`: the row's reviewed template expanded with
    /// the injected `READ`, the `ERROR-FILE` the row names and the E-44
    /// codes of a database failure ([`file_error_text`]).
    FileError,
}

/// One `COCRDLIC` row of the validation audit, as a request proves it.
struct Audited {
    /// The row ID, recorded once a request has proven the row.
    id: &'static str,
    /// The `Status` column.
    status: StatusCode,
    /// The `kind=` of the trigger: `None` for a 200 page.
    kind: Option<ErrorKind>,
    /// The `BMS field` column, `—` when the row names none.
    field: String,
    /// The `Order` column: the rank in which the program rejects the input.
    order: u16,
    /// The text the answer carries.
    message: String,
    /// The first double-quoted input of the trigger's `example=`, if any.
    example: Option<String>,
}

impl Audited {
    /// The row `id` of `doc`, its text checked against `text`.
    fn load(doc: &Document, id: &'static str, text: Text) -> Self {
        let (table, row) = doc
            .find_row(id)
            .unwrap_or_else(|| panic!("the validation audit defines no {id} row"));
        let cell = |name: &str| {
            table
                .cell(row, name)
                .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
                .trim()
        };
        let status = cell("Status")
            .parse::<u16>()
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {}", cell("Status")));
        let order = cell("Order")
            .parse()
            .unwrap_or_else(|_| panic!("{id} has no numeric Order: {}", cell("Order")));
        let trigger = key_values(cell("Trigger condition"));
        let message = match text {
            Text::Literal(catalog) => {
                let quoted = code_span(cell("Message text")).map(|span| span.text);
                assert_eq!(
                    quoted,
                    Some(format!("\"{catalog}\"")),
                    "{id}: the audited text and the catalog text differ"
                );
                catalog.to_owned()
            }
            Text::FileError => file_error_text(doc, id, error_file(id, cell("Message text"))),
        };
        Self {
            id,
            status,
            kind: kind_of(id, pair(&trigger, id, "kind")),
            field: cell("BMS field").to_owned(),
            order,
            message,
            example: trigger
                .iter()
                .find(|entry| entry.key == "example")
                .and_then(|entry| entry.value.split('"').nth(1))
                .map(str::to_owned),
        }
    }

    /// The `code` of the error body this row answers with.
    fn code(&self) -> &'static str {
        self.kind
            .unwrap_or_else(|| panic!("{} sets no error flag, so it answers no error", self.id))
            .code()
    }

    /// The `(field, message)` entry of the error body this row answers with.
    fn entry(&self) -> (&str, &str) {
        (self.field.as_str(), self.message.as_str())
    }

    /// The input of the row's example.
    fn example(&self) -> &str {
        self.example
            .as_deref()
            .unwrap_or_else(|| panic!("{} gives no quoted example input", self.id))
    }
}

/// The [`ErrorKind`] whose name is the `kind=` value `name` of row `id`, or
/// `None` for [`NO_KIND`].
fn kind_of(id: &str, name: &str) -> Option<ErrorKind> {
    if name == NO_KIND {
        return None;
    }
    let kind = KINDS
        .into_iter()
        .find(|kind| format!("{kind:?}") == name)
        .unwrap_or_else(|| panic!("{id} records an unknown kind {name:?}"));
    Some(kind)
}

/// The `ERROR-FILE` literal the message cell of row `id` names: the quoted
/// text after its last `ERROR-FILE`, for example `CARDDAT ` in
/// `(…, ERROR-FILE 'CARDDAT ', E-44)`.
fn error_file<'a>(id: &str, cell: &'a str) -> &'a str {
    cell.rsplit_once("ERROR-FILE")
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
        .map_or_else(
            || panic!("{id} names no quoted ERROR-FILE: {cell}"),
            |(file, _)| file,
        )
}

/// The `WS-FILE-ERROR-MESSAGE` [app/cbl/COCRDLIC.cbl:L153-171] the
/// `WHEN OTHER` branch of `9000-READ-FORWARD` or `9100-READ-BACKWARDS` moves
/// to `WS-ERROR-MSG`: the reviewed template of row `id` of `doc`, its operands
/// bound to `READ`, the `ERROR-FILE` literal `file` and the E-44 `RESP` and
/// `RESP2`, each laid out in its picture, and cut at the 75 bytes of the
/// receiver.
fn file_error_text(doc: &Document, id: &str, file: &str) -> String {
    Template::load(doc, id).expand(
        &[
            Operand::alphanumeric("ERROR-OPNAME", ERROR_OPNAME_READ, ERROR_OPNAME_WIDTH),
            Operand::alphanumeric("ERROR-FILE", file, ERROR_FILE_WIDTH),
            Operand::unsigned_to_alphanumeric(
                "ERROR-RESP",
                RESP_IOERR,
                RESP_DIGITS,
                ERROR_RESP_WIDTH,
            ),
            Operand::unsigned_to_alphanumeric(
                "ERROR-RESP2",
                RESP2_IOERR,
                RESP_DIGITS,
                ERROR_RESP_WIDTH,
            ),
        ],
        ERROR_MSG_WIDTH,
    )
}

/// The value of the first `key=` pair of row `id`'s structured cell `pairs`.
fn pair<'a>(pairs: &'a [KeyValue], id: &str, key: &str) -> &'a str {
    pairs.iter().find(|entry| entry.key == key).map_or_else(
        || panic!("{id} records no `{key}=`"),
        |entry| entry.value.as_str(),
    )
}

/// The `COCRDLIC` rows of the validation audit, each with the text it shows.
struct Rows {
    /// `2210-EDIT-ACCOUNT`: the account filter is not 11 digits.
    account_filter: Audited,
    /// `2220-EDIT-CARD`: the card filter is not 16 digits.
    card_filter: Audited,
    /// `2250-EDIT-ARRAY`: more than one row selected.
    two_selected: Audited,
    /// `2250-EDIT-ARRAY`: a selector other than `S` or `U`.
    bad_action: Audited,
    /// PF8 after the last page was shown.
    no_more_pages: Audited,
    /// The first page keeps no card.
    no_records: Audited,
    /// The end of the file after at least one kept card.
    no_more_records: Audited,
    /// A failed `READNEXT`.
    forward_failure: Audited,
    /// PF7 on the first page.
    no_previous: Audited,
    /// A failed `READPREV`.
    backward_failure: Audited,
}

impl Rows {
    /// The ten rows of `doc`, each text checked against the catalog.
    fn load(doc: &Document) -> Self {
        Self {
            account_filter: Audited::load(
                doc,
                "V-COCRDLIC-001",
                Text::Literal(messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT),
            ),
            card_filter: Audited::load(
                doc,
                "V-COCRDLIC-002",
                Text::Literal(messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT),
            ),
            two_selected: Audited::load(
                doc,
                "V-COCRDLIC-003",
                Text::Literal(messages::PLEASE_SELECT_ONLY_ONE_RECORD_TO_VIEW_OR_UPDATE),
            ),
            bad_action: Audited::load(
                doc,
                "V-COCRDLIC-004",
                Text::Literal(messages::INVALID_ACTION_CODE),
            ),
            no_more_pages: Audited::load(
                doc,
                "V-COCRDLIC-005",
                Text::Literal(messages::NO_MORE_PAGES_TO_DISPLAY_COCRDLIC),
            ),
            no_records: Audited::load(
                doc,
                "V-COCRDLIC-006",
                Text::Literal(messages::NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC),
            ),
            no_more_records: Audited::load(
                doc,
                "V-COCRDLIC-007",
                Text::Literal(messages::NO_MORE_RECORDS_TO_SHOW),
            ),
            forward_failure: Audited::load(doc, "V-COCRDLIC-008", Text::FileError),
            no_previous: Audited::load(
                doc,
                "V-COCRDLIC-009",
                Text::Literal(messages::NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC),
            ),
            backward_failure: Audited::load(doc, "V-COCRDLIC-010", Text::FileError),
        }
    }
}

/// One `CARD-RECORD` of the fixture, with the fields a list row shows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FixtureCard {
    /// `CARD-NUM`, the `CARDDAT` key.
    number: String,
    /// `CARD-ACCT-ID`, 11 digits.
    account: String,
    /// `CARD-ACTIVE-STATUS`.
    status: String,
}

impl FixtureCard {
    /// The list row fields of `record`, a `CARD-RECORD` decoded at the
    /// reviewed section A offsets of `CVACT02Y`: `card_number`
    /// (P-CVACT02Y-001), `card_acct_id` (P-CVACT02Y-002) and
    /// `card_active_status` (P-CVACT02Y-006).
    fn decoded(record: &Record) -> Self {
        Self {
            number: record.value("card_number").to_owned(),
            account: record.value("card_acct_id").to_owned(),
            status: record.value("card_active_status").to_owned(),
        }
    }

    /// The list row `GET /cards` answers for this card: `CRDNUMn`, `ACCTNOn`
    /// and `CRDSTSn` (P-COCRDLIC-017).
    fn summary(&self) -> Value {
        json!({
            "card_number": self.number,
            "account_id": self.account,
            "card_active_status": self.status,
        })
    }
}

/// The seeded cards in `CARDDAT` key order, and the fixture rows of the audit
/// they agree with.
struct Fixture {
    /// Every card, in ascending card number: byte order, the order of the KSDS
    /// key.
    cards: Vec<FixtureCard>,
    /// The cards a screen shows, `WS-MAX-SCREEN-LINES`.
    rows: usize,
    /// The one card of account `00000000050`.
    account_card: FixtureCard,
    /// An account no card belongs to.
    missing_account: String,
}

impl Fixture {
    /// Decodes every `CARD-RECORD` of the card fixture at the section A
    /// offsets of `parity`, counts them against [`CARD_COUNT_ROW`], and checks
    /// them against the fixture rows of `parity`.
    fn load(parity: &Document) -> Self {
        let records = fixture_records::Layout::card(parity).every_record();
        assert_eq!(
            fixture_records::section_d_value(parity, CARD_COUNT_ROW, "table"),
            CARD_TABLE,
            "{CARD_COUNT_ROW} table"
        );
        let count: usize = fixture_records::section_d_value(parity, CARD_COUNT_ROW, "rows")
            .parse()
            .unwrap_or_else(|error| panic!("{CARD_COUNT_ROW} rows is not a count: {error}"));
        assert_eq!(
            records.len(),
            count,
            "{CARD_COUNT_ROW}: the CARD-RECORDs of the card fixture"
        );
        let mut cards: Vec<FixtureCard> = records.iter().map(FixtureCard::decoded).collect();
        cards.sort_by(|left, right| left.number.cmp(&right.number));
        let browse = result_pairs(parity, BROWSE_ROW);
        let rows = pair(&browse, BROWSE_ROW, "page_size")
            .parse()
            .unwrap_or_else(|error| panic!("{BROWSE_ROW} page_size is not a count: {error}"));
        let card_row = result_pairs(parity, ACCOUNT_CARD_ROW);
        let account_card = FixtureCard {
            number: quoted(ACCOUNT_CARD_ROW, pair(&card_row, ACCOUNT_CARD_ROW, "value")),
            account: pair(&card_row, ACCOUNT_CARD_ROW, "card_acct_id").to_owned(),
            status: pair(&card_row, ACCOUNT_CARD_ROW, "card_active_status").to_owned(),
        };
        let missing = result_pairs(parity, MISSING_ACCOUNT_ROW);
        let fixture = Self {
            cards,
            rows,
            account_card,
            missing_account: quoted(
                MISSING_ACCOUNT_ROW,
                pair(&missing, MISSING_ACCOUNT_ROW, "value"),
            ),
        };
        fixture.agrees_with_the_fixture_rows(parity);
        fixture
    }

    /// The fixture holds the first page P-CVACT02Y-012 lists, the one card of
    /// P-CVACT02Y-009's account, and no card of P-CVACT01Y-016's account.
    fn agrees_with_the_fixture_rows(&self, parity: &Document) {
        let first = result_pairs(parity, FIRST_PAGE_ROW);
        let shown = &self.cards[..self.rows];
        let listed = |key: &str| listed(pair(&first, FIRST_PAGE_ROW, key));
        assert_eq!(
            pair(&first, FIRST_PAGE_ROW, "rows").parse::<usize>().ok(),
            Some(self.rows),
            "{FIRST_PAGE_ROW} rows against {BROWSE_ROW} page_size"
        );
        let numbers: Vec<&str> = shown.iter().map(|card| card.number.as_str()).collect();
        assert_eq!(
            numbers,
            listed("card_number"),
            "{FIRST_PAGE_ROW} card_number"
        );
        let accounts: Vec<&str> = shown.iter().map(|card| card.account.as_str()).collect();
        assert_eq!(
            accounts,
            listed("card_acct_id"),
            "{FIRST_PAGE_ROW} card_acct_id"
        );
        let status = listed("card_active_status");
        assert!(
            shown.iter().all(|card| status == [card.status.as_str()]),
            "{FIRST_PAGE_ROW} card_active_status {status:?} on every row"
        );
        assert_eq!(
            self.of_account(&self.account_card.account),
            slice::from_ref(&self.account_card),
            "{ACCOUNT_CARD_ROW}: the one card of its account"
        );
        assert!(
            self.of_account(&self.missing_account).is_empty(),
            "{MISSING_ACCOUNT_ROW}: no card of its account"
        );
        assert!(
            self.cards
                .iter()
                .all(|card| card.number.as_str() < PAST_LAST_CARD),
            "{PAST_LAST_CARD} lies past every card"
        );
    }

    /// The pages of the unfiltered forward browse, a screen each.
    fn pages(&self) -> Vec<&[FixtureCard]> {
        self.cards.chunks(self.rows).collect()
    }

    /// The cards of `account`, in key order.
    fn of_account(&self, account: &str) -> Vec<FixtureCard> {
        self.cards
            .iter()
            .filter(|card| card.account == account)
            .cloned()
            .collect()
    }

    /// The card number after `number` in key order.
    fn following(&self, number: &str) -> &str {
        self.cards
            .iter()
            .map(|card| card.number.as_str())
            .find(|next| *next > number)
            .unwrap_or_else(|| panic!("no card follows {number}"))
    }

    /// Page `index` of the unfiltered forward browse, without a message: its
    /// cards, the first card of the next page as `next_key` (null on the last
    /// page), and its own first card as `prev_key`.
    fn forward_page(&self, index: usize) -> Value {
        let pages = self.pages();
        let page = pages[index];
        let next = pages.get(index + 1).map(|next| next[0].number.as_str());
        page_body(page, next, Some(page[0].number.as_str()))
    }
}

/// The `Expected result` pairs of the parity row `id`.
fn result_pairs(parity: &Document, id: &str) -> Vec<KeyValue> {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit defines no {id} row"));
    let cell = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    key_values(cell)
}

/// The text of the code span `value` of row `id`, without its double quotes.
fn quoted(id: &str, value: &str) -> String {
    code_span(value).map_or_else(
        || panic!("{id} quotes no value in {value:?}"),
        |span| span.text.trim_matches('"').to_owned(),
    )
}

/// The comma-separated values before the first space of `value`, for example
/// the card numbers of `0500…,0683… (rows 1 to 7)`.
fn listed(value: &str) -> Vec<&str> {
    value
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split(',')
        .collect()
}

/// A page body without a message: `items`, `next_key` and `prev_key`, each
/// key `null` when absent.
fn page_body(items: &[FixtureCard], next_key: Option<&str>, prev_key: Option<&str>) -> Value {
    let items: Vec<Value> = items.iter().map(FixtureCard::summary).collect();
    json!({ "items": items, "next_key": next_key, "prev_key": prev_key })
}

/// The status and the body of a page that shows the text of `row`, or none.
///
/// A row that sets no error flag answers its 200 page with the text in
/// `message`; a page without a text omits `message`.
fn with_text(mut body: Value, row: Option<&Audited>) -> (StatusCode, Value) {
    match row {
        Some(row) => {
            body["message"] = Value::from(row.message.as_str());
            (row.status, body)
        }
        None => (StatusCode::OK, body),
    }
}

/// Asserts that `resp`, the answer to `GET path`, is `status` with exactly
/// the body `expected`: no field more, none less.
fn assert_page(path: &str, resp: &Resp, status: StatusCode, expected: &Value) {
    let (actual, body, raw) = resp;
    assert_eq!(*actual, status, "status of GET {path}: {raw}");
    assert_eq!(body, expected, "body of GET {path}: {raw}");
}

/// The names of the query parameters the `OpenAPI` document declares for
/// `GET /cards`.
fn list_parameters() -> BTreeSet<String> {
    let doc = openapi();
    doc["paths"][CARDS]["get"]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("the OpenAPI document declares no parameters for GET {CARDS}"))
        .iter()
        .map(|parameter| {
            parameter["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a GET {CARDS} parameter has no name: {parameter}"))
                .to_owned()
        })
        .collect()
}

/// The `OpenAPI` document the router publishes.
fn openapi() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The HTTP methods the `OpenAPI` document `doc` declares on `path`.
fn operations(doc: &Value, path: &str) -> BTreeSet<String> {
    doc["paths"][path]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document declares no path {path}"))
        .keys()
        .filter(|key| HTTP_METHODS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// The parsed `docs/parity/function-map.md`.
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The `Endpoint` and `UI control` cells of the function map row `id`.
fn function_row(map: &Document, id: &str) -> (String, String) {
    let (table, row) = map
        .find_row(id)
        .unwrap_or_else(|| panic!("the function map defines no {id} row"));
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
            .trim()
            .to_owned()
    };
    (cell("Endpoint"), cell("UI control"))
}

/// The method, the path template and the query parameter of the function
/// map `Endpoint` cell `endpoint` of row `id`, for example `GET`,
/// `/cards/{id}` and `account_id` of `GET /cards/{id}?account_id=`.
fn endpoint_parts<'a>(id: &str, endpoint: &'a str) -> (&'a str, &'a str, &'a str) {
    let parts = endpoint.split_once(' ').and_then(|(method, target)| {
        let (template, query) = target.split_once('?')?;
        Some((method, template, query.strip_suffix('=')?))
    });
    parts.unwrap_or_else(|| panic!("{id} names no `METHOD /path?parameter=` endpoint: {endpoint}"))
}

/// Every AID key `YYYY-STORE-PFKEY` stores: ENTER, CLEAR, PA1 and PA2, then
/// PF1 to PF24.
fn aid_keys() -> Vec<String> {
    NAMED_AID_KEYS
        .iter()
        .map(|key| (*key).to_owned())
        .chain((1..=PF_KEY_COUNT).map(|number| format!("PF{number}")))
        .collect()
}

/// Whether `token` is a version token: [`VERSION_LEN`] lower-case hex
/// digits.
fn is_version(token: &str) -> bool {
    token.len() == VERSION_LEN
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The string `key` of the JSON object `item`, answered by `GET path`.
fn text_of<'a>(item: &'a Value, key: &str, path: &str) -> &'a str {
    item[key]
        .as_str()
        .unwrap_or_else(|| panic!("an item of GET {path} has no string {key}: {item}"))
}

/// The requests of one signed-on user against the seeded app, and the audit
/// rows they have proven.
struct Suite<'a> {
    app: &'a TestApp,
    token: String,
    rows: &'a Rows,
    fixture: &'a Fixture,
    parity: &'a Document,
    proven: RefCell<BTreeSet<&'static str>>,
}

impl Suite<'_> {
    /// Records `ids` as proven by the request just asserted.
    fn prove(&self, ids: &[&'static str]) {
        self.proven.borrow_mut().extend(ids.iter().copied());
    }

    /// Sends `GET /cards?query` with the user's token; an empty `query` sends
    /// no query string.
    async fn get(&self, query: &str) -> (String, Resp) {
        let path = if query.is_empty() {
            CARDS.to_owned()
        } else {
            format!("{CARDS}?{query}")
        };
        let resp = self.app.get(&path, Some(&self.token)).await;
        (path, resp)
    }

    /// Sends `GET /cards?query` with the user's token and `Accept: accept`,
    /// and returns the path, the answer's `Content-Type` and the answer.
    async fn get_as(&self, query: &str, accept: &str) -> (String, String, Resp) {
        let path = format!("{CARDS}?{query}");
        let response = self
            .app
            .client()
            .get(self.app.url(&path))
            .bearer_auth(&self.token)
            .header(ACCEPT, accept)
            .send()
            .await
            .unwrap_or_else(|error| panic!("GET {path} failed: {error}"));
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let raw = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("the body of GET {path} cannot be read: {error}"));
        let body = serde_json::from_str(&raw).unwrap_or(Value::Null);
        (path, content_type, (status, body, raw))
    }

    /// Asserts that `GET /cards?query` answers the page `body` with the text
    /// of `row`, or with none.
    async fn expect_page(&self, query: &str, body: Value, row: Option<&Audited>) {
        let (path, resp) = self.get(query).await;
        let (status, expected) = with_text(body, row);
        assert_page(&path, &resp, status, &expected);
        if let Some(row) = row {
            self.prove(&[row.id]);
        }
    }

    /// Asserts that `GET /cards?query` answers the error of `row`: its status,
    /// its kind's code, its text, and the `fields` entries in their order.
    async fn expect_error(&self, query: &str, row: &Audited, fields: &[(&str, &str)]) {
        let (_, resp) = self.get(query).await;
        assert_error(&resp, row.status, row.code(), &row.message, fields);
        self.prove(&[row.id]);
    }

    /// COCRDLIC 9000-READ-FORWARD, 1400-SETUP-MESSAGE: ENTER, then PF8 from
    /// each page's `next_key` until no page follows. The pages concatenate to
    /// every card exactly once, in key order, a screen each; the last one
    /// shows the end-of-file text.
    async fn forward_walk(&self) {
        let pages = self.fixture.pages();
        let mut start: Option<String> = None;
        let mut shown: Vec<String> = Vec::new();
        for index in 0..pages.len() {
            let query = start.as_deref().map_or_else(String::new, |key| {
                format!("start_key={key}&direction=forward")
            });
            let text = (index + 1 == pages.len()).then_some(&self.rows.no_more_records);
            let (path, resp) = self.get(&query).await;
            let (status, expected) = with_text(self.fixture.forward_page(index), text);
            assert_page(&path, &resp, status, &expected);
            let items = resp.1["items"].as_array().cloned().unwrap_or_default();
            shown.extend(
                items
                    .iter()
                    .filter_map(|item| item["card_number"].as_str())
                    .map(str::to_owned),
            );
            start = resp.1["next_key"].as_str().map(str::to_owned);
        }
        assert_eq!(start, None, "the last page offers no next page");
        let every: Vec<&str> = self
            .fixture
            .cards
            .iter()
            .map(|card| card.number.as_str())
            .collect();
        assert_eq!(
            shown, every,
            "the forward pages show every card once, in key order"
        );
        self.prove(&[
            "V-COCRDLIC-007",
            BROWSE_ROW,
            FIRST_PAGE_ROW,
            "P-COCRDLIC-004",
            "P-COCRDLIC-012",
            "P-COCRDLIC-014",
            "P-COCRDLIC-015",
            "P-COCRDLIC-016",
            "P-COCRDLIC-017",
            "P-COCRDLIC-020",
        ]);
    }

    /// COCRDLIC 9100-READ-BACKWARDS, 1400-SETUP-MESSAGE: PF7 from the first
    /// card of each page gives the page before it, back to the first page,
    /// which shows 'NO PREVIOUS PAGES TO DISPLAY'; PF7 on the first page, from
    /// its `prev_key` or with no key, re-reads it with that text.
    async fn backward_walk(&self) {
        let pages = self.fixture.pages();
        for index in (0..pages.len() - 1).rev() {
            let start = &pages[index + 1][0].number;
            let text = (index == 0).then_some(&self.rows.no_previous);
            let query = format!("start_key={start}&direction=backward");
            self.expect_page(&query, self.fixture.forward_page(index), text)
                .await;
        }
        let first = &pages[0][0].number;
        for query in [
            format!("start_key={first}&direction=backward"),
            "direction=backward".to_owned(),
        ] {
            let first_page = self.fixture.forward_page(0);
            self.expect_page(&query, first_page, Some(&self.rows.no_previous))
                .await;
        }
        self.prove(&["P-COCRDLIC-011", "P-COCRDLIC-013"]);
    }

    /// COCRDLIC 1400-SETUP-MESSAGE and the page size: PF8 past the last card
    /// shows 'NO MORE PAGES TO DISPLAY', whether the key is past every card or
    /// the `next_key` the look-ahead `READNEXT` found beyond the filter's last
    /// card; `page_size` takes 1 to 100 rows and refuses 0 and 101.
    async fn page_boundaries(&self) {
        let rows = self.rows;
        let account = &self.fixture.account_card.account;
        for filter in [String::new(), format!("account_id={account}&")] {
            let query = format!("{filter}start_key={PAST_LAST_CARD}&direction=forward");
            let empty = page_body(&[], None, Some(PAST_LAST_CARD));
            self.expect_page(&query, empty, Some(&rows.no_more_pages))
                .await;
        }

        // The look-ahead read after a full page is not filtered
        // [app/cbl/COCRDLIC.cbl:L1191-1221], so the page of account 1's one
        // card offers the next card of the file, and PF8 there finds none.
        let one = self.fixture.of_account(ACCOUNT_ONE);
        let card = one[0].number.as_str();
        let following = self.fixture.following(card);
        let query = format!("account_id={ACCOUNT_ONE}&page_size=1");
        self.expect_page(&query, page_body(&one, Some(following), Some(card)), None)
            .await;
        let query = format!("{query}&start_key={following}&direction=forward");
        let empty = page_body(&[], None, Some(following));
        self.expect_page(&query, empty, Some(&rows.no_more_pages))
            .await;

        let first = self.fixture.cards[0].number.as_str();
        let everything = page_body(&self.fixture.cards, None, Some(first));
        let query = format!("page_size={MAX_PAGE_SIZE}");
        self.expect_page(&query, everything, Some(&rows.no_more_records))
            .await;
        for size in [0, MAX_PAGE_SIZE + 1] {
            let (path, (status, body, raw)) = self.get(&format!("page_size={size}")).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "status of GET {path}: {raw}"
            );
            assert_eq!(
                body["code"].as_str(),
                Some(ErrorKind::Invalid.code()),
                "code of GET {path}: {raw}"
            );
        }
        self.prove(&["P-COCRDLIC-003", "P-COCRDLIC-011"]);
    }

    /// COCRDLIC 9500-FILTER-RECORDS: a valid filter keeps only its cards, the
    /// end of the file after them shows 'NO MORE RECORDS TO SHOW', and a first
    /// page that keeps none shows 'NO RECORDS FOUND FOR THIS SEARCH
    /// CONDITION.' with `items: []`.
    async fn filters(&self) {
        let rows = self.rows;
        let one = self.fixture.of_account(ACCOUNT_ONE);
        assert_eq!(one.len(), 1, "account {ACCOUNT_ONE} holds one card");
        let only = |cards: &[FixtureCard]| page_body(cards, None, Some(cards[0].number.as_str()));
        self.expect_page(
            &format!("account_id={ACCOUNT_ONE}"),
            only(&one),
            Some(&rows.no_more_records),
        )
        .await;

        let card = slice::from_ref(&self.fixture.account_card);
        let (number, account) = (&card[0].number, &card[0].account);
        assert_eq!(
            rows.no_more_records.example(),
            account.as_str(),
            "the V-COCRDLIC-007 example is the account of {ACCOUNT_CARD_ROW}"
        );
        for query in [
            format!("account_id={account}"),
            format!("card_number={number}"),
            format!("account_id={account}&card_number={number}"),
        ] {
            self.expect_page(&query, only(card), Some(&rows.no_more_records))
                .await;
        }

        let missing = rows.no_records.example();
        assert_eq!(
            missing, self.fixture.missing_account,
            "the V-COCRDLIC-006 example is the account of {MISSING_ACCOUNT_ROW}"
        );
        for query in [
            format!("account_id={missing}"),
            format!("account_id={ACCOUNT_ONE}&card_number={number}"),
        ] {
            self.expect_page(&query, page_body(&[], None, None), Some(&rows.no_records))
                .await;
        }

        // V-COCRDLIC-009 example: PF7 from the first card of account 50.
        let query = format!("account_id={account}&start_key={number}&direction=backward");
        self.expect_page(&query, only(card), Some(&rows.no_previous))
            .await;
        self.prove(&[
            "P-COCRDLIC-004",
            "P-COCRDLIC-012",
            ACCOUNT_CARD_ROW,
            MISSING_ACCOUNT_ROW,
        ]);
    }

    /// COCRDLIC 2100-RECEIVE-SCREEN, 2210-EDIT-ACCOUNT, 2220-EDIT-CARD: an
    /// absent, empty, space, zero or `LOW-VALUES` filter of its field's width
    /// is not supplied, so the first page lists every card.
    async fn blank_filters(&self) {
        let queries = [
            "account_id=".to_owned(),
            "card_number=".to_owned(),
            "account_id=&card_number=".to_owned(),
            format!("account_id={}", "%20".repeat(ACCOUNT_FILTER_WIDTH)),
            format!("card_number={}", "%20".repeat(CARD_FILTER_WIDTH)),
            format!("account_id={}", "0".repeat(ACCOUNT_FILTER_WIDTH)),
            format!("card_number={}", "0".repeat(CARD_FILTER_WIDTH)),
            format!("account_id={}", "%00".repeat(ACCOUNT_FILTER_WIDTH)),
        ];
        for query in &queries {
            self.expect_page(query, self.fixture.forward_page(0), None)
                .await;
        }
        self.prove(&["P-COCRDLIC-022"]);
    }

    /// COCRDLIC 2200-EDIT-INPUTS: a filter that is neither blank nor a number
    /// of its field's width is answered with its text on its field, forward
    /// and backward alike, before any card is read. With both filters failing
    /// the row of the lower `Order` supplies `message`, and each field keeps
    /// its own text, in `Order`. A filter longer than its field is refused
    /// with no text.
    async fn filter_edits(&self) {
        let account = &self.rows.account_filter;
        let card = &self.rows.card_filter;
        let not_accounts = [account.example()].into_iter().chain(NOT_ACCOUNT_NUMBERS);
        for value in not_accounts {
            for direction in ["forward", "backward"] {
                let query = format!("account_id={value}&direction={direction}");
                self.expect_error(&query, account, &[account.entry()]).await;
            }
        }
        for value in [card.example(), SHORT_CARD_NUMBER] {
            for direction in ["forward", "backward"] {
                let query = format!("card_number={value}&direction={direction}");
                self.expect_error(&query, card, &[card.entry()]).await;
            }
        }

        let mut failing = [account, card];
        failing.sort_by_key(|row| row.order);
        let entries: Vec<(&str, &str)> = failing.iter().map(|row| row.entry()).collect();
        let query = format!(
            "account_id={}&card_number={SHORT_CARD_NUMBER}",
            NOT_ACCOUNT_NUMBERS[0]
        );
        self.expect_error(&query, failing[0], &entries).await;

        let query = format!(
            "account_id={}&card_number={}",
            "0".repeat(ACCOUNT_FILTER_WIDTH + 1),
            "0".repeat(CARD_FILTER_WIDTH + 1)
        );
        let (_, resp) = self.get(&query).await;
        let over_width: Vec<(&str, &str)> = failing
            .iter()
            .map(|row| (row.field.as_str(), NO_MESSAGE))
            .collect();
        assert_error(
            &resp,
            StatusCode::BAD_REQUEST,
            ErrorKind::Invalid.code(),
            NO_MESSAGE,
            &over_width,
        );
        self.prove(&[
            "P-COCRDLIC-005",
            "P-COCRDLIC-006",
            "P-COCRDLIC-019",
            "P-COCRDLIC-021",
            "P-COCRDLIC-023",
        ]);
    }

    /// COCRDLIC 2250-EDIT-ARRAY: the row selectors have no request parameter
    /// (E-54). The `OpenAPI` document declares only the filters and the
    /// pagination, and selectors sent anyway are not read: the first page
    /// answers as it does without them, so neither selector text can be shown.
    async fn selectors(&self) {
        assert_eq!(
            list_parameters(),
            BTreeSet::from(LIST_PARAMETERS.map(String::from)),
            "the query parameters of GET {CARDS}"
        );
        for (query, row) in [
            (TWO_SELECTED, &self.rows.two_selected),
            (BAD_ACTION, &self.rows.bad_action),
        ] {
            let (path, resp) = self.get(query).await;
            assert_page(&path, &resp, StatusCode::OK, &self.fixture.forward_page(0));
            assert!(
                !resp.2.contains(row.message.as_str()),
                "GET {path} shows the {} text: {}",
                row.id,
                resp.2
            );
            self.prove(&[row.id]);
        }
        self.prove(&[
            "P-COCRDLIC-007",
            "P-COCRDLIC-008",
            "P-COCRDLIC-018",
            "P-COCRDLIC-022",
        ]);
    }

    /// COCRDLIC 0000-MAIN, YYYY-STORE-PFKEY — P-COCRDLIC-001, P-COCRDLIC-024
    ///
    /// No AID key reaches `GET /cards` (E-56): the `OpenAPI` document declares
    /// query parameters only, none of them a key, and an [`AID_PARAMETER`]
    /// sent anyway is not read. Every key `YYYY-STORE-PFKEY` stores, PF13 to
    /// PF24 included, answers what ENTER answers, the first page without a
    /// text, so no key is treated as invalid and none shows the invalid-key
    /// text. From a `start_key` the page follows `direction` alone, the one
    /// control of each request: PF7 and PF19, which `YYYY-STORE-PFKEY` stores
    /// as PF7, page no request backward, and PF8 and PF20 turn no backward
    /// request forward.
    async fn aid_keys(&self) {
        let doc = openapi();
        let declared = doc["paths"][CARDS]["get"]["parameters"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("the OpenAPI document declares no parameters for GET {CARDS}")
            });
        for parameter in declared {
            assert_eq!(
                parameter["in"].as_str(),
                Some("query"),
                "P-COCRDLIC-024: GET {CARDS} takes its controls as query parameters only: {parameter}"
            );
        }
        assert!(
            !list_parameters().contains(AID_PARAMETER),
            "P-COCRDLIC-024: GET {CARDS} declares no AID key"
        );
        let invalid_key = messages::CCDA_MSG_INVALID_KEY.trim_end();
        let first = self.fixture.forward_page(0);
        for key in aid_keys() {
            let (path, resp) = self.get(&format!("{AID_PARAMETER}={key}")).await;
            assert_page(&path, &resp, StatusCode::OK, &first);
            assert!(
                !resp.2.contains(invalid_key),
                "P-COCRDLIC-001: GET {path} shows the invalid-key text: {}",
                resp.2
            );
        }
        let pages = self.fixture.pages();
        let second = pages[1][0].number.as_str();
        for key in ["PF7", "PF19"] {
            let query = format!("start_key={second}&{AID_PARAMETER}={key}");
            self.expect_page(&query, self.fixture.forward_page(1), None)
                .await;
        }
        for key in ["PF8", "PF20"] {
            let query = format!("start_key={second}&direction=backward&{AID_PARAMETER}={key}");
            let first_page = self.fixture.forward_page(0);
            self.expect_page(&query, first_page, Some(&self.rows.no_previous))
                .await;
        }
        self.prove(&["P-COCRDLIC-001", "P-COCRDLIC-024"]);
    }

    /// COCRDLIC 0000-MAIN PF3 — P-COCRDLIC-002
    ///
    /// PF3's `XCTL` to the menu becomes navigation (F-COCRDLI-002): the
    /// function map gives the key a link to the menu's route and no
    /// endpoint, the `OpenAPI` document gives `/cards` no operation besides
    /// `GET`, and a PF3 sent anyway is not read, so the request answers the
    /// first page and transfers nowhere.
    async fn pf3_is_navigation(&self) {
        let (endpoint, control) = function_row(&function_map(), PF3_CONTROL);
        assert_eq!(
            endpoint, NAVIGATION,
            "P-COCRDLIC-002: {PF3_CONTROL} gives PF3 no endpoint"
        );
        assert_eq!(
            pair(&key_values(&control), PF3_CONTROL, "target"),
            MENU_ROUTE,
            "P-COCRDLIC-002: {PF3_CONTROL} leads PF3 to the menu"
        );
        assert_eq!(
            operations(&openapi(), CARDS),
            BTreeSet::from(["get".to_owned()]),
            "P-COCRDLIC-002: {CARDS} has no operation for PF3"
        );
        let (path, resp) = self.get(&format!("{AID_PARAMETER}=PF3")).await;
        assert_page(&path, &resp, StatusCode::OK, &self.fixture.forward_page(0));
        self.prove(&["P-COCRDLIC-002"]);
    }

    /// COCRDLIC 0000-MAIN VIEW-REQUESTED-ON, UPDATE-REQUESTED-ON —
    /// P-COCRDLIC-009, P-COCRDLIC-010
    ///
    /// Selectors `S` and `U` become the row actions of F-COCRDLI-005 and
    /// F-COCRDLI-006 (E-54): the view route and the edit route of a row are
    /// two routes of its card, and each fetches the card through the endpoint
    /// both rows name, `GET /cards/{id}` with the row's card number and the
    /// row's account as `account_id`, as the `XCTL` hands `COCRDSLC` and
    /// `COCRDUPC` the row's account and card number. The edit route saves
    /// through `PUT` on the same path, whose body takes that account and the
    /// token the fetch returned. Every row of the first page opens its own
    /// card: the number, account and status the row shows, and a token.
    async fn row_actions(&self) {
        let map = function_map();
        let mut targets = BTreeSet::new();
        let mut endpoints = BTreeSet::new();
        for id in ROW_ACTIONS {
            let (endpoint, control) = function_row(&map, id);
            let target = pair(&key_values(&control), id, "target").to_owned();
            assert!(
                target.starts_with(CARD_ROUTE),
                "P-COCRDLIC-009, P-COCRDLIC-010: {id} opens {target}, not a route of the row's card"
            );
            targets.insert(target);
            endpoints.insert(endpoint);
        }
        assert_eq!(
            targets.len(),
            ROW_ACTIONS.len(),
            "P-COCRDLIC-010: the view and the edit route differ: {targets:?}"
        );
        let endpoints: Vec<String> = endpoints.into_iter().collect();
        let [endpoint] = endpoints.as_slice() else {
            panic!("{ROW_ACTIONS:?} name different endpoints: {endpoints:?}");
        };
        let (method, template, parameter) = endpoint_parts(ROW_ACTIONS[0], endpoint);
        assert_eq!(
            method, "GET",
            "P-COCRDLIC-009: the row actions fetch the card"
        );
        let doc = openapi();
        let methods = operations(&doc, template);
        assert!(
            methods.contains("get") && methods.contains("put"),
            "P-COCRDLIC-010: the edit route fetches and saves through {template}: {methods:?}"
        );
        let body = &doc["components"]["schemas"][UPDATE_SCHEMA]["properties"];
        for key in [parameter, VERSION_KEY] {
            assert!(
                body.get(key).is_some(),
                "P-COCRDLIC-010: PUT {template} takes {key}: {body}"
            );
        }
        let (path, (status, page, raw)) = self.get("").await;
        assert_eq!(status, StatusCode::OK, "status of GET {path}: {raw}");
        let items = page["items"]
            .as_array()
            .unwrap_or_else(|| panic!("GET {path} answers no items: {raw}"));
        assert_eq!(items.len(), self.fixture.rows, "rows of GET {path}: {raw}");
        for item in items {
            let number = text_of(item, "card_number", &path);
            let account = text_of(item, "account_id", &path);
            let card_path = format!("{}?{parameter}={account}", template.replace("{id}", number));
            let (status, card, raw) = self.app.get(&card_path, Some(&self.token)).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "P-COCRDLIC-009: GET {card_path}: {raw}"
            );
            for key in ROW_KEYS {
                assert_eq!(
                    card[key], item[key],
                    "P-COCRDLIC-009, P-COCRDLIC-010: GET {card_path} opens the card of its row: {raw}"
                );
            }
            assert!(
                card[VERSION_KEY].as_str().is_some_and(is_version),
                "P-COCRDLIC-010: GET {card_path} gives the edit route a version token: {raw}"
            );
        }
        self.prove(&["P-COCRDLIC-009", "P-COCRDLIC-010"]);
    }

    /// COCRDLIC SEND-PLAIN-TEXT, SEND-LONG-TEXT — P-COCRDLIC-025,
    /// P-COCRDLIC-026, P-COCRDLIC-027, P-COCRDLIC-028
    ///
    /// No path reaches either paragraph (E-67), and nothing derives from
    /// them: the `OpenAPI` document holds no card path besides the list and
    /// the card, `/cards` has no operation besides `GET`, and every answer it
    /// declares is JSON. A request that asks for plain text gets the JSON
    /// page, or the JSON error body of a failing filter, never a `SEND TEXT`
    /// of `WS-ERROR-MSG` or `WS-LONG-MSG`.
    async fn no_plain_text(&self) {
        let doc = openapi();
        let paths: BTreeSet<&str> = doc["paths"]
            .as_object()
            .unwrap_or_else(|| panic!("the OpenAPI document declares no paths"))
            .keys()
            .map(String::as_str)
            .filter(|path| *path == CARDS || path.starts_with("/cards/"))
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from(CARD_PATHS),
            "P-COCRDLIC-025, P-COCRDLIC-026: no card endpoint derives from SEND-PLAIN-TEXT or SEND-LONG-TEXT"
        );
        assert_eq!(
            operations(&doc, CARDS),
            BTreeSet::from(["get".to_owned()]),
            "P-COCRDLIC-027, P-COCRDLIC-028: {CARDS} has its GET only"
        );
        let responses = doc["paths"][CARDS]["get"]["responses"]
            .as_object()
            .unwrap_or_else(|| panic!("the OpenAPI document declares no answers of GET {CARDS}"));
        for (status, response) in responses {
            let types: BTreeSet<&str> = response["content"]
                .as_object()
                .unwrap_or_else(|| panic!("GET {CARDS} declares {status} without content"))
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                types,
                BTreeSet::from([JSON]),
                "P-COCRDLIC-025 to P-COCRDLIC-028: GET {CARDS} answers {status} as JSON only"
            );
        }
        let (path, content_type, resp) = self.get_as("", PLAIN_TEXT).await;
        assert!(
            content_type.starts_with(JSON),
            "P-COCRDLIC-025, P-COCRDLIC-027: GET {path} asking for {PLAIN_TEXT} answers {content_type:?}"
        );
        assert_page(&path, &resp, StatusCode::OK, &self.fixture.forward_page(0));
        let account = &self.rows.account_filter;
        let query = format!("account_id={}", account.example());
        let (path, content_type, resp) = self.get_as(&query, PLAIN_TEXT).await;
        assert!(
            content_type.starts_with(JSON),
            "P-COCRDLIC-026, P-COCRDLIC-028: GET {path} asking for {PLAIN_TEXT} answers {content_type:?}"
        );
        assert_error(
            &resp,
            account.status,
            account.code(),
            &account.message,
            &[account.entry()],
        );
        self.prove(&[
            "P-COCRDLIC-025",
            "P-COCRDLIC-026",
            "P-COCRDLIC-027",
            "P-COCRDLIC-028",
        ]);
    }

    /// COCRDLIC 1250-SETUP-ARRAY-ATTRIBS — P-COCRDLIC-030
    ///
    /// The row-1 marker [`MARKER_ROW`] quotes is never shown (E-67), and no
    /// selector reaches the API (E-54): a row-1 selector that is empty, a
    /// space, `LOW-VALUES` or the marker itself is not read, so the first page
    /// answers exactly as without it, each item carrying its account, number
    /// and status only, and no value of an item is the marker.
    async fn row_one_marker(&self) {
        let disposition = result_pairs(self.parity, MARKER_ROW);
        let marker = quoted(MARKER_ROW, pair(&disposition, MARKER_ROW, "disposition"));
        let first = self.fixture.forward_page(0);
        for selector in ROW_ONE_SELECTORS {
            let (path, resp) = self.get(&format!("{ROW_ONE_SELECTOR}={selector}")).await;
            assert_page(&path, &resp, StatusCode::OK, &first);
            let shown = resp.1["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .flat_map(|item| item.values());
            assert!(
                shown
                    .map(Value::as_str)
                    .all(|value| value != Some(marker.as_str())),
                "P-COCRDLIC-030: GET {path} shows the row-1 marker {marker:?}: {}",
                resp.2
            );
        }
        self.prove(&[MARKER_ROW]);
    }

    /// COCRDLIC 0000-MAIN after the EVALUATE — P-COCRDLIC-029
    ///
    /// The `IF INPUT-ERROR` after the EVALUATE is not carried (E-67): an input
    /// error is answered by `WHEN INPUT-ERROR` itself, its text in the error
    /// body on its field, forward and backward. That branch skips
    /// `9000-READ-FORWARD` for a filter that is not OK, so its answer reads no
    /// card: it is the same with the card master hidden and once it is back,
    /// when the list reads again. The tail would have returned with no map.
    async fn input_errors_end_in_their_branch(&self) {
        let account = &self.rows.account_filter;
        let card = &self.rows.card_filter;
        let failing = [
            (format!("account_id={}", account.example()), account),
            (format!("card_number={}", card.example()), card),
        ];
        for sql in [HIDE_CARDS, RESTORE_CARDS] {
            self.app.stack().exec_sql(sql).await;
            for (filter, row) in &failing {
                for direction in ["forward", "backward"] {
                    let query = format!("{filter}&direction={direction}");
                    self.expect_error(&query, row, &[row.entry()]).await;
                }
            }
        }
        self.expect_page("", self.fixture.forward_page(0), None)
            .await;
        self.prove(&["P-COCRDLIC-029"]);
    }

    /// COCRDLIC 9000-READ-FORWARD and 9100-READ-BACKWARDS `WHEN OTHER`: with
    /// the card master hidden, a failed `READNEXT` or `READPREV` answers the
    /// `CARDDAT` file-error text on its field. PF7 on the first page still
    /// shows 'NO PREVIOUS PAGES TO DISPLAY', whose lower `Order` outranks the
    /// failed re-read's text, and a failing filter skips the browse.
    async fn read_failures(&self) {
        let rows = self.rows;
        let second_page = &self.fixture.pages()[1][0].number;
        self.app.stack().exec_sql(HIDE_CARDS).await;

        let forward = &rows.forward_failure;
        self.expect_error("", forward, &[forward.entry()]).await;
        let backward = &rows.backward_failure;
        let query = format!("start_key={second_page}&direction=backward");
        self.expect_error(&query, backward, &[backward.entry()])
            .await;
        assert!(
            rows.no_previous.order < forward.order,
            "{} must outrank {}",
            rows.no_previous.id,
            forward.id
        );
        let empty = page_body(&[], None, None);
        self.expect_page("direction=backward", empty, Some(&rows.no_previous))
            .await;
        let account = &rows.account_filter;
        let query = format!("account_id={}", NOT_ACCOUNT_NUMBERS[0]);
        self.expect_error(&query, account, &[account.entry()]).await;

        self.app.stack().exec_sql(RESTORE_CARDS).await;
        self.expect_page("", self.fixture.forward_page(0), None)
            .await;
        self.prove(&["P-COCRDLIC-021", "P-COCRDLIC-023"]);
    }
}

/// Every `COCRDLIC` row of the validation audit is proven, and no other; every
/// parity row cited is defined.
fn assert_rows_proven(proven: &BTreeSet<&'static str>, parity: &Document) {
    let defined: BTreeSet<String> = common::validation_ids(PROGRAM).into_iter().collect();
    let exercised: BTreeSet<String> = proven
        .iter()
        .copied()
        .filter(|id| classify_id(id) == Some(IdKind::Validation))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        exercised, defined,
        "the {PROGRAM} rows of the validation audit this file exercises"
    );
    for id in proven
        .iter()
        .filter(|id| classify_id(id) == Some(IdKind::Parity))
    {
        assert!(
            parity.find_row(id).is_some(),
            "the parity audit defines no {id} row"
        );
    }
}

/// `GET /cards` pages, filters and answers every `COCRDLIC` row as the audits
/// and the fixture give them.
#[test]
fn ccli_card_list_pages_filters_and_texts_follow_cocrdlic() {
    let parity = common::parity_audit();
    let rows = Rows::load(&common::validation_audit());
    let fixture = Fixture::load(&parity);
    let app = TestApp::start();
    let proven = block_on(async {
        let suite = Suite {
            app: &app,
            token: app.user_token().await,
            rows: &rows,
            fixture: &fixture,
            parity: &parity,
            proven: RefCell::default(),
        };
        suite.forward_walk().await;
        suite.backward_walk().await;
        suite.page_boundaries().await;
        suite.filters().await;
        suite.blank_filters().await;
        suite.filter_edits().await;
        suite.selectors().await;
        suite.aid_keys().await;
        suite.pf3_is_navigation().await;
        suite.row_actions().await;
        suite.no_plain_text().await;
        suite.row_one_marker().await;
        suite.input_errors_end_in_their_branch().await;
        suite.read_failures().await;
        suite.proven.into_inner()
    });
    assert_rows_proven(&proven, &parity);
}
