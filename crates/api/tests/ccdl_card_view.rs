//! CCDL, the credit card view (`COCRDSLC`, map `COCRDSL`), as
//! `GET /cards/{id}?account_id=`: the card number is the `{id}` path segment
//! and the account number the `account_id` query parameter (E-55).
//!
//! Every request crosses the real router, so the bearer token, the
//! extractors, the status mapping and the `{code, message, fields}` body are
//! those the SPA meets. The expectations come from the audits, never from
//! literals of this file:
//!
//! - each `V-COCRDSLC-nnn` row gives the status, the kind, the BMS field and
//!   the order of its answer, and its message text, which must equal the
//!   `carddemo_domain::messages` constant the rule returns;
//! - P-COCRDSLC-010 gives the information line of a found card, which must
//!   equal its `carddemo_domain::messages` constant too;
//! - section A of the parity audit gives the `CARD-RECORD` offsets each
//!   fixture card of `app/data/ASCII/carddata.txt` is decoded by, and section
//!   D the seeded example card and the missing card.
//!
//! The edit table is keyed by V-ID. Each case lists every row whose trigger
//! its input meets, and the answer it expects must be the one the audit's
//! precedence gives: the failing row with the lowest order, except
//! `NO-SEARCH-CRITERIA-RECEIVED` (V-COCRDSLC-005), which replaces the
//! account text when both keys are blank. The test fails unless the table
//! and the read-failure case together exercise every COCRDSLC row the
//! validation audit defines, and unless every audit row it relies on has
//! passed its review.
//!
//! Values holding a non-digit in a key are E-47 inputs (`halt`): the zero
//! test of the `9(n)` redefine is unpinned for them. The cases sending such
//! values pin the outcome E-47 proposes and the audit rows' own examples use
//! (V-COCRDSLC-002, V-COCRDSLC-004), which the rule implements, so a changed
//! decision shows here as a failing case.
//!
//! The dialogue the 3270 conducts around those edits is proven too: every
//! AID key acts as ENTER, no state survives a request, the header, the
//! attributes and the text sends have no counterpart, the account search is
//! never performed, and an abend is the 500 of the error mapping.
//!
//! Docker must be running: the test starts one seeded `postgres:16.15`
//! container. The read failures (V-COCRDSLC-007) are injected near the end,
//! by hiding the card table (SQLSTATE `42P01` on the card read), and undone
//! each time; last, the pool is closed for the abend.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use axum::extract::State;
use carddemo_api::AppState;
use carddemo_api::auth::{AuthUser, UserType};
use carddemo_api::dto::cards::{CardViewField, CardViewQuery};
use carddemo_api::error::{ApiPath, ApiQuery};
use carddemo_api::handlers::cards::get_card;
use carddemo_audit::{Document, code_span, key_values};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;
use carddemo_testkit::{block_on, fixtures};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, CONTENT_TYPE, SET_COOKIE};
use serde_json::{Map, Value};

use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COCRDSLC";

/// Account number absent, blank, `*` or all zeros.
const V001: &str = "V-COCRDSLC-001";
/// Account number not 11 digits.
const V002: &str = "V-COCRDSLC-002";
/// Card number blank, `*` or all zeros.
const V003: &str = "V-COCRDSLC-003";
/// Card number not 16 digits.
const V004: &str = "V-COCRDSLC-004";
/// Both keys blank.
const V005: &str = "V-COCRDSLC-005";
/// No card master record for the card number.
const V006: &str = "V-COCRDSLC-006";
/// The card master read fails.
const V007: &str = "V-COCRDSLC-007";

/// The catalog text each row with a literal message returns; V-COCRDSLC-007
/// is composed by [`messages::file_error_message`] instead.
const CATALOG: [(&str, &str); 6] = [
    (V001, messages::ACCOUNT_NUMBER_NOT_PROVIDED),
    (V002, messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT),
    (V003, messages::CARD_NUMBER_NOT_PROVIDED),
    (V004, messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT),
    (V005, messages::NO_INPUT_RECEIVED),
    (V006, messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION),
];

/// The parity rows this file proves, each checked for a passing review:
/// the key edits (P-COCRDSLC-002, P-COCRDSLC-003), the not-found and
/// file-error reads (P-COCRDSLC-004, P-COCRDSLC-005), the card shown
/// (P-COCRDSLC-006), the information line and the keys of every send
/// (P-COCRDSLC-010), the field each error marks (P-COCRDSLC-011), the read
/// skipped after a key error (P-COCRDSLC-013), the keys received from the path
/// and the query (P-COCRDSLC-014), the `*` and spaces handling and the cross
/// edit (P-COCRDSLC-015), and the read by card number alone (P-COCRDSLC-016);
/// then the `CARD-RECORD` layout and the seeded cards.
const PARITY_ROWS: [&str; 20] = [
    "P-COCRDSLC-002",
    "P-COCRDSLC-003",
    "P-COCRDSLC-004",
    "P-COCRDSLC-005",
    "P-COCRDSLC-006",
    SCREEN_VARS_ROW,
    "P-COCRDSLC-011",
    "P-COCRDSLC-013",
    "P-COCRDSLC-014",
    "P-COCRDSLC-015",
    "P-COCRDSLC-016",
    LAYOUT_CARD_NUM,
    LAYOUT_ACCT_ID,
    LAYOUT_CVV,
    LAYOUT_EMBOSSED_NAME,
    LAYOUT_EXPIRAION_DATE,
    LAYOUT_ACTIVE_STATUS,
    SEEDED_CARDS,
    EXISTING_CARD,
    MISSING_CARD_ROW,
];

/// The parity rows of the dialogue this file proves, each checked for a
/// passing review: every key acting as ENTER (P-COCRDSLC-001), the stateless
/// return (P-COCRDSLC-007), the read after the edits and the read-only view
/// (P-COCRDSLC-008), the screen header (P-COCRDSLC-009), the send
/// (P-COCRDSLC-012), the account search never performed (P-COCRDSLC-017,
/// P-COCRDSLC-021), the texts never sent (P-COCRDSLC-018, P-COCRDSLC-019,
/// P-COCRDSLC-022), the abend (P-COCRDSLC-020, P-COCRDSLC-023), the tail of
/// the EVALUATE (P-COCRDSLC-024) and the information-line attribute
/// (P-COCRDSLC-025); then the account no card belongs to.
const DIALOGUE_ROWS: [&str; 15] = [
    "P-COCRDSLC-001",
    "P-COCRDSLC-007",
    "P-COCRDSLC-008",
    SCREEN_INIT_ROW,
    "P-COCRDSLC-012",
    ACCOUNT_SEARCH_ROW,
    "P-COCRDSLC-018",
    PLAIN_TEXT_ROW,
    "P-COCRDSLC-020",
    "P-COCRDSLC-021",
    "P-COCRDSLC-022",
    ABEND_ROW,
    "P-COCRDSLC-024",
    "P-COCRDSLC-025",
    MISSING_ACCOUNT_ROW,
];

/// `1100-SCREEN-INIT`: the header fields and the values moved to them.
const SCREEN_INIT_ROW: &str = "P-COCRDSLC-009";
/// `1200-SETUP-SCREEN-VARS`: the keys, the information line and the error
/// text of every send; it quotes the information line of a found card.
const SCREEN_VARS_ROW: &str = "P-COCRDSLC-010";
/// The 88-level `9100-GETCARD-BYACCTCARD` sets on `RESP NORMAL`, which
/// [`SCREEN_VARS_ROW`] names before the found card's `info=`.
const FOUND_CARD_FLAG: &str = "FOUND-CARDS-FOR-ACCOUNT";
/// The result key [`SCREEN_VARS_ROW`] gives the information line under.
const INFO_KEY: &str = "info";
/// `9150-GETCARD-BYACCT`, never performed: it quotes the not-found text of
/// the account search.
const ACCOUNT_SEARCH_ROW: &str = "P-COCRDSLC-017";
/// `SEND-PLAIN-TEXT`: it quotes the text of `WHEN OTHER`.
const PLAIN_TEXT_ROW: &str = "P-COCRDSLC-019";
/// `ABEND-ROUTINE`: it quotes the text `ABEND-MSG` never holds.
const ABEND_ROW: &str = "P-COCRDSLC-023";
/// The account number no seeded account, and so no card, has, section D.
const MISSING_ACCOUNT_ROW: &str = "P-CVACT01Y-016";

/// The header fields of [`SCREEN_INIT_ROW`] given a literal.
const HEADER_LITERALS: [&str; 2] = ["TRNNAMEO", "PGMNAMEO"];

/// The header fields of [`SCREEN_INIT_ROW`] given a `COTTL01Y` title.
const HEADER_TITLES: [&str; 2] = ["TITLE01O", "TITLE02O"];

/// The copybook of the screen titles [app/cpy/COTTL01Y.cpy:L18-22].
const TITLE_COPYBOOK: &str = "app/cpy/COTTL01Y.cpy";

/// The column of a COBOL comment line's `*`, zero-based: column 7.
const COMMENT_COLUMN: usize = 6;

/// The query parameter a 3270 AID key is sent in, named after `EIBAID`,
/// which `GET /cards/{id}` does not declare: no AID key reaches the API
/// (E-56).
const AID_PARAMETER: &str = "EIBAID";

/// The AID keys `YYYY-STORE-PFKEY` stores under their own name besides the
/// PF keys [app/cpy/CSSTRPFY.cpy:L22-29].
const NAMED_AID_KEYS: [&str; 4] = ["ENTER", "CLEAR", "PA1", "PA2"];

/// PF1 to PF24, the PF keys `YYYY-STORE-PFKEY` stores, PF13 to PF24 as PF1
/// to PF12 [app/cpy/CSSTRPFY.cpy:L30-77].
const PF_KEY_COUNT: u8 = 24;

/// The query parameter a program context is sent in, named after
/// `CDEMO-PGM-CONTEXT 9(01)` [app/cpy/COCOM01Y.cpy:L29-31], which no request
/// carries.
const CONTEXT_PARAMETER: &str = "CDEMO-PGM-CONTEXT";

/// Program contexts other than `CDEMO-PGM-ENTER` (0) and `CDEMO-PGM-REENTER`
/// (1), the contexts of the `WHEN OTHER` that performs `SEND-PLAIN-TEXT`.
const OTHER_CONTEXTS: [&str; 2] = ["2", "9"];

/// The `OpenAPI` path of the card view.
const VIEW_PATH: &str = "/cards/{id}";

/// The parameters `GET /cards/{id}` declares, as name and location: the card
/// number in the path and the account number in the query.
const VIEW_PARAMETERS: [(&str, &str); 2] = [("id", "path"), ("account_id", "query")];

/// The answers `GET /cards/{id}` declares, each with the schema of its body.
const VIEW_ANSWERS: [(&str, &str); 5] = [
    ("200", "CardViewResponse"),
    ("400", "ErrorBody"),
    ("401", "ErrorBody"),
    ("404", "ErrorBody"),
    ("500", "ErrorBody"),
];

/// The media type of every answer of the API.
const JSON: &str = "application/json";

/// The media type of a CICS `SEND TEXT`, which a request may ask for.
const PLAIN_TEXT: &str = "text/plain";

/// The keys of an error body.
const ERROR_KEYS: [&str; 3] = ["code", "message", "fields"];

/// The keys of one `fields` entry of an error body.
const FIELD_KEYS: [&str; 2] = ["field", "message"];

/// Card `9680294154603697` of [`OTHER_ACCOUNT`]
/// [app/data/ASCII/carddata.txt:L49].
const OTHER_CARD: &str = "9680294154603697";

/// Every stored card, every column, in key order: what the view must leave
/// unchanged.
const CARDS_SNAPSHOT: &str = "SELECT * FROM cards ORDER BY card_number";

/// The SPA's navigation definition, whose menu options name their programs.
const MENU_FILE: &str = "web/src/nav/menu.ts";

/// The signed-on user of a direct handler call: the seeded regular user.
const SIGNED_ON_USER: &str = "USER0001";

/// `ABEND-CODE` of `WHEN OTHER` and the `ABCODE` of `ABEND-ROUTINE`
/// [app/cbl/COCRDSLC.cbl:L375] [app/cbl/COCRDSLC.cbl:L875-876], which no
/// answer carries.
const ABEND_CODES: [&str; 2] = ["0001", "9999"];

/// `CARD-NUM X(16)`, section A.
const LAYOUT_CARD_NUM: &str = "P-CVACT02Y-001";
/// `CARD-ACCT-ID 9(11)`, section A.
const LAYOUT_ACCT_ID: &str = "P-CVACT02Y-002";
/// `CARD-CVV-CD 9(03)`, section A: part of the record, never of the response.
const LAYOUT_CVV: &str = "P-CVACT02Y-003";
/// `CARD-EMBOSSED-NAME X(50)`, section A.
const LAYOUT_EMBOSSED_NAME: &str = "P-CVACT02Y-004";
/// `CARD-EXPIRAION-DATE X(10)`, section A.
const LAYOUT_EXPIRAION_DATE: &str = "P-CVACT02Y-005";
/// `CARD-ACTIVE-STATUS X(01)`, section A.
const LAYOUT_ACTIVE_STATUS: &str = "P-CVACT02Y-006";
/// The seeded card count, section D.
const SEEDED_CARDS: &str = "P-CVACT02Y-008";
/// The seeded example card the COCRDSLC rows use, section D.
const EXISTING_CARD: &str = "P-CVACT02Y-009";
/// The card number no seeded card has, section D.
const MISSING_CARD_ROW: &str = "P-CVACT02Y-010";

/// The response keys of the fields `1200-SETUP-SCREEN-VARS` shows, each with
/// the section A row of its `CARD-RECORD` field (P-COCRDSLC-006).
const SHOWN_FIELDS: [(&str, &str); 5] = [
    ("card_number", LAYOUT_CARD_NUM),
    ("account_id", LAYOUT_ACCT_ID),
    ("card_embossed_name", LAYOUT_EMBOSSED_NAME),
    ("card_expiraion_date", LAYOUT_EXPIRAION_DATE),
    ("card_active_status", LAYOUT_ACTIVE_STATUS),
];

/// The response key of the card's version token.
const VERSION_KEY: &str = "version";

/// The response key of the information line shown with a found card.
const MESSAGE_KEY: &str = "message";

/// The hex digits of a version token: 16 lower-case hex characters.
const VERSION_LEN: usize = 16;

/// The card view route.
const CARDS: &str = "/cards";

/// The fixture file of `CARD-RECORD`, under `app/data/ASCII`.
const CARD_FILE: &str = "carddata.txt";

/// The card of [`EXISTING_CARD`], `0500024453765740`: the example card of the
/// COCRDSLC rows [app/data/ASCII/carddata.txt:L1].
const EXAMPLE_CARD: &str = "0500024453765740";

/// The account of [`EXAMPLE_CARD`].
const EXAMPLE_ACCOUNT: &str = "00000000050";

/// The card number of [`MISSING_CARD_ROW`].
const MISSING_CARD: &str = "9999999999999999";

/// An account other than [`EXAMPLE_ACCOUNT`], the account of card
/// `9680294154603697` [app/data/ASCII/carddata.txt:L49].
const OTHER_ACCOUNT: &str = "00000000001";

/// An account number of 11 zeros, which `2210-EDIT-ACCOUNT` reads as not
/// provided through its `9(11)` redefine.
const ZERO_ACCOUNT: &str = "00000000000";

/// A card number of 16 zeros, which `2220-EDIT-CARD` reads as not provided
/// through its `9(16)` redefine, before any read.
const ZERO_CARD: &str = "0000000000000000";

/// An account number one character longer than `ACCTSIDI X(11)`.
const OVER_WIDTH_ACCOUNT: &str = "000000000050";

/// The `message` of a key longer than its map field: no COBOL path can send
/// one, so the answer carries no text.
const NO_MESSAGE: &str = "";

/// The `code` of a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The label the V-COCRDSLC-007 note names the `ERROR-OPNAME` literal after,
/// the `'READ'` of `9100-GETCARD-BYACCTCARD` [app/cbl/COCRDSLC.cbl:L767].
const OPNAME_LABEL: &str = "ERROR-OPNAME";

/// The label the V-COCRDSLC-007 note names the `ERROR-FILE` literal after,
/// `LIT-CARDFILENAME` [app/cbl/COCRDSLC.cbl:L768].
const FILE_LABEL: &str = "ERROR-FILE";

/// The `RESP` E-44 sets for a database failure: `IOERR`.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 sets alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// Hides the card master: the card read then fails with SQLSTATE `42P01`.
const HIDE_CARDS: &str = "ALTER TABLE cards RENAME TO cards_offline";

/// Puts the card master back.
const RESTORE_CARDS: &str = "ALTER TABLE cards_offline RENAME TO cards";

/// One request of the edit table.
struct Case {
    /// The row whose answer the request gets.
    answer: &'static str,
    /// Every row whose trigger condition the input meets.
    fired: &'static [&'static str],
    /// The `{id}` path segment, percent-encoded.
    card: &'static str,
    /// The `account_id` query value, percent-encoded; `None` sends none.
    account: Option<&'static str>,
}

/// The edit and not-found cases, keyed by the V-ID of their answer. `*` and
/// spaces are sent percent-encoded; a key of spaces or `*` is the
/// `LOW-VALUES` of `2200-EDIT-MAP-INPUTS` (P-COCRDSLC-015).
///
/// All zeros is a key not provided, so `00000000000` and `0000000000000000`
/// get the blank-key texts, never the not-numeric text or a read. The
/// not-numeric keys hold a non-digit or a padding space (E-47, `halt`) and
/// expect the outcome E-47 proposes, as the audit examples do.
const CASES: [Case; 22] = [
    case(V001, &[V001], EXAMPLE_CARD, None),
    case(V001, &[V001], EXAMPLE_CARD, Some("")),
    case(V001, &[V001], EXAMPLE_CARD, Some("%20%20%20")),
    case(V001, &[V001], EXAMPLE_CARD, Some("%2A")),
    case(V001, &[V001], EXAMPLE_CARD, Some(ZERO_ACCOUNT)),
    case(V001, &[V001, V004], "050002445376574X", None),
    case(V002, &[V002], EXAMPLE_CARD, Some("0000000005A")),
    case(V002, &[V002], EXAMPLE_CARD, Some("ABC")),
    case(V002, &[V002], EXAMPLE_CARD, Some("12345")),
    case(V002, &[V002, V003], "%20%20%20", Some("0000000005A")),
    case(V002, &[V002, V004], "050002445376574X", Some("0000000005A")),
    case(V002, &[V002], MISSING_CARD, Some("0000000005A")),
    case(V003, &[V003], "%20%20%20", Some(EXAMPLE_ACCOUNT)),
    case(V003, &[V003], ZERO_CARD, Some(EXAMPLE_ACCOUNT)),
    case(V003, &[V003], "%2A", Some(OTHER_ACCOUNT)),
    case(V004, &[V004], "050002445376574X", Some(EXAMPLE_ACCOUNT)),
    case(V004, &[V004], "12345", Some(EXAMPLE_ACCOUNT)),
    case(V004, &[V004], "05000244537657400", Some(EXAMPLE_ACCOUNT)),
    case(V005, &[V001, V003, V005], "%20", None),
    case(V005, &[V001, V003, V005], "%2A", Some("%2A")),
    case(V005, &[V001, V003, V005], ZERO_CARD, Some(ZERO_ACCOUNT)),
    case(V006, &[V006], MISSING_CARD, Some(EXAMPLE_ACCOUNT)),
];

/// A [`Case`], so that the table above stays one line per request.
const fn case(
    answer: &'static str,
    fired: &'static [&'static str],
    card: &'static str,
    account: Option<&'static str>,
) -> Case {
    Case {
        answer,
        fired,
        card,
        account,
    }
}

/// The answer one validation audit row gives.
struct Answer {
    /// The row's ID.
    id: String,
    /// The `Status` cell.
    status: StatusCode,
    /// The `code` of the row's `kind=`, as `ErrorKind::code` publishes it.
    code: &'static str,
    /// The message text, from the catalog once it equals the audit's text.
    message: String,
    /// The `BMS field` cell.
    field: String,
    /// The `Order` cell.
    order: u32,
}

/// The answers of the COCRDSLC rows, by V-ID.
type Answers = BTreeMap<String, Answer>;

/// The answer of every COCRDSLC row of `doc`, each row checked for a passing
/// review and for a BMS field the `CardViewField` enum publishes.
fn answers(doc: &Document) -> Answers {
    let published = [
        CardViewField::AcctSid.as_str(),
        CardViewField::CardSid.as_str(),
    ];
    validation_ids(PROGRAM)
        .into_iter()
        .map(|id| {
            assert_reviewed(doc, &id);
            let answer = answer(doc, &id);
            assert!(
                published.contains(&answer.field.as_str()),
                "{id} marks {}, which `CardViewField` does not publish",
                answer.field
            );
            (id, answer)
        })
        .collect()
}

/// The answer the validation audit row `id` gives.
fn answer(doc: &Document, id: &str) -> Answer {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
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
        .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {:?}", cell("Status")));
    let Some(kind) = key_values(cell("Trigger condition"))
        .into_iter()
        .find(|pair| pair.key == "kind")
    else {
        panic!("{id} names no kind= in its trigger condition");
    };
    let order = cell("Order")
        .parse()
        .unwrap_or_else(|_| panic!("{id} has no numeric order: {:?}", cell("Order")));
    Answer {
        id: id.to_owned(),
        status,
        code: kind_named(&kind.value, id).code(),
        message: message(id, cell("Message text")),
        field: cell("BMS field").to_owned(),
        order,
    }
}

/// The [`ErrorKind`] whose variant is `name`, as the audit's `kind=` writes it.
fn kind_named(name: &str, id: &str) -> ErrorKind {
    [
        ErrorKind::Invalid,
        ErrorKind::Empty,
        ErrorKind::NotFound,
        ErrorKind::Duplicate,
        ErrorKind::Conflict,
        ErrorKind::SignonFailed,
        ErrorKind::Unexpected,
    ]
    .into_iter()
    .find(|kind| format!("{kind:?}") == name)
    .unwrap_or_else(|| panic!("{id} names an unknown kind {name:?}"))
}

/// The message the row `id` answers with, given its `Message text` cell.
///
/// A literal text must equal its [`CATALOG`] constant byte for byte. The
/// read-failure text is [`messages::file_error_message`] of the `ERROR-OPNAME`
/// and `ERROR-FILE` literals the cell's note names, with the RESP and RESP2
/// E-44 sets for a database failure.
fn message(id: &str, cell: &str) -> String {
    if id == V007 {
        return messages::file_error_message(
            quoted_after(cell, OPNAME_LABEL),
            quoted_after(cell, FILE_LABEL),
            RESP_IOERR,
            RESP2_IOERR,
        );
    }
    let (_, catalog) = CATALOG
        .iter()
        .find(|(row, _)| *row == id)
        .unwrap_or_else(|| panic!("no catalog text is bound to {id}"));
    let audited = code_span(cell).map(|span| unquoted(&span.text));
    assert_eq!(
        audited.as_deref(),
        Some(*catalog),
        "{id}: the catalog text differs from the audited message"
    );
    (*catalog).to_owned()
}

/// The text between the single quotes that follow the last `label` of
/// `cell`, for example `CARDDAT ` in `ERROR-FILE 'CARDDAT '`.
fn quoted_after<'a>(cell: &'a str, label: &str) -> &'a str {
    let Some((text, _)) = cell
        .rsplit_once(label)
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
    else {
        panic!("no quoted {label} in the message cell: {cell}");
    };
    text
}

/// `text` without the double quotes the audit writes around an exact text.
fn unquoted(text: &str) -> String {
    text.strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or_else(|| panic!("the audit text {text:?} is not double-quoted"))
        .to_owned()
}

/// Asserts that the last review-log verdict on the row `id` of `doc` is
/// `pass`: only reviewed rows gate this test.
fn assert_reviewed(doc: &Document, id: &str) {
    let verdict = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(|row| table.cell(row, "Row ID") == Some(id))
                .filter_map(|row| table.cell(row, "Verdict"))
        })
        .next_back();
    assert_eq!(verdict, Some("pass"), "{id} has not passed its review");
}

/// The value of `key=` in the `Expected result` cell of the parity row `id`,
/// without the double quotes of an exact text.
fn result_value(doc: &Document, id: &str, key: &str) -> String {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let cell = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    let Some(pair) = key_values(cell).into_iter().find(|pair| pair.key == key) else {
        panic!("{id} gives no {key}= in its expected result: {cell}");
    };
    match code_span(&pair.value) {
        Some(span) => unquoted(&span.text),
        None => pair.value,
    }
}

/// One field of `CARD-RECORD` as section A of the parity audit places it.
struct Field {
    /// The response key that carries the field.
    key: &'static str,
    /// The `Offset` cell: the field's first byte in the record.
    offset: usize,
    /// The `Length` cell.
    length: usize,
}

/// The section A placement of the field of the parity row `id`, carried
/// under `key`.
fn field(doc: &Document, id: &str, key: &'static str) -> Field {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let number = |name: &str| {
        table
            .cell(row, name)
            .and_then(|cell| cell.trim().parse::<usize>().ok())
            .unwrap_or_else(|| panic!("{id} has no numeric `{name}` cell"))
    };
    Field {
        key,
        offset: number("Offset"),
        length: number("Length"),
    }
}

/// One fixture card as the view must show it.
struct FixtureCard {
    /// The shown fields in [`SHOWN_FIELDS`] order, as response key and value:
    /// the `X(n)` fields right-trimmed, the 11 digits of the account as stored.
    shown: Vec<(&'static str, String)>,
    /// `CARD-CVV-CD`, which no response value may carry.
    cvv: String,
}

impl FixtureCard {
    /// The shown value under the response key `key`.
    fn value(&self, key: &str) -> &str {
        let Some((_, value)) = self.shown.iter().find(|(name, _)| *name == key) else {
            panic!("a fixture card shows no {key}");
        };
        value
    }
}

/// Every record of `carddata.txt`, decoded by the section A placements of
/// `doc`; their count must be the seeded card count of [`SEEDED_CARDS`].
fn fixture_cards(doc: &Document) -> Vec<FixtureCard> {
    let shown: Vec<Field> = SHOWN_FIELDS
        .iter()
        .map(|(key, id)| field(doc, id, key))
        .collect();
    let cvv = field(doc, LAYOUT_CVV, "card_cvv_cd");
    let path = fixtures::ascii_dir().join(CARD_FILE);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let cards: Vec<FixtureCard> = text
        .lines()
        .enumerate()
        .map(|(index, record)| FixtureCard {
            shown: shown
                .iter()
                .map(|placed| (placed.key, bytes_of(record, placed, index)))
                .collect(),
            cvv: bytes_of(record, &cvv, index),
        })
        .collect();
    let rows = result_value(doc, SEEDED_CARDS, "rows");
    assert_eq!(
        cards.len().to_string(),
        rows,
        "{CARD_FILE} holds the seeded card count of {SEEDED_CARDS}"
    );
    cards
}

/// The bytes `placed` names in `record`, line `index + 1` of the fixture,
/// right-trimmed of the spaces that pad an `X(n)` field.
fn bytes_of(record: &str, placed: &Field, index: usize) -> String {
    let end = placed.offset + placed.length;
    record
        .get(placed.offset..end)
        .unwrap_or_else(|| {
            panic!(
                "{CARD_FILE} line {} holds no bytes {}..{end}",
                index + 1,
                placed.offset
            )
        })
        .trim_end_matches(' ')
        .to_owned()
}

/// The section D response keys of the seeded example card: each response key
/// with the `key=` of the [`EXISTING_CARD`] row that pins its value.
const EXAMPLE_KEYS: [(&str, &str); 5] = [
    ("card_number", "value"),
    ("account_id", "card_acct_id"),
    ("card_embossed_name", "card_embossed_name"),
    ("card_expiraion_date", "card_expiraion_date"),
    ("card_active_status", "card_active_status"),
];

/// The seeded example card as section D pins it, checked against its fixture
/// record and the example keys of the edit table; the missing card must be
/// [`MISSING_CARD`] and absent from the fixture.
fn example_card(doc: &Document, cards: &[FixtureCard]) -> Vec<(&'static str, String)> {
    let example: Vec<(&'static str, String)> = EXAMPLE_KEYS
        .iter()
        .map(|(key, pinned)| (*key, result_value(doc, EXISTING_CARD, pinned)))
        .collect();
    let fixture = cards
        .iter()
        .find(|card| card.value("card_number") == EXAMPLE_CARD)
        .unwrap_or_else(|| panic!("{CARD_FILE} holds no card {EXAMPLE_CARD}"));
    assert_eq!(
        fixture.shown, example,
        "{EXISTING_CARD} pins the fixture record of {EXAMPLE_CARD}"
    );
    assert_eq!(fixture.value("account_id"), EXAMPLE_ACCOUNT);
    assert_eq!(result_value(doc, MISSING_CARD_ROW, "value"), MISSING_CARD);
    assert!(
        cards
            .iter()
            .all(|card| card.value("card_number") != MISSING_CARD),
        "{CARD_FILE} must hold no card {MISSING_CARD}"
    );
    example
}

/// The router path of the card view of `card`, with `account` as the
/// `account_id` query value when given.
fn view_path(card: &str, account: Option<&str>) -> String {
    if let Some(account) = account {
        format!("{CARDS}/{card}?account_id={account}")
    } else {
        format!("{CARDS}/{card}")
    }
}

/// The answer row `id` of `answers`.
fn answer_of<'a>(answers: &'a Answers, id: &str) -> &'a Answer {
    answers
        .get(id)
        .unwrap_or_else(|| panic!("the validation audit defines no {id} row"))
}

/// The row whose answer an input meeting the triggers of `fired` gets: the
/// one with the lowest (order, ID), except V-COCRDSLC-005, whose
/// `SET NO-SEARCH-CRITERIA-RECEIVED` is unconditional once both keys are
/// blank (P-COCRDSLC-015).
fn winner(fired: &[&'static str], answers: &Answers) -> &'static str {
    if fired.contains(&V005) {
        assert!(
            fired.contains(&V001) && fired.contains(&V003),
            "{V005} fires only when both blank-key rows fire"
        );
        return V005;
    }
    fired
        .iter()
        .copied()
        .min_by_key(|id| (answer_of(answers, id).order, *id))
        .unwrap_or_else(|| panic!("a case fires no row"))
}

/// Asserts that `resp`, the answer to `GET path`, is exactly the answer of
/// its row: status, code and message, at the row's BMS field.
fn assert_answer(resp: &Resp, answer: &Answer, path: &str) {
    assert_eq!(
        resp.0, answer.status,
        "GET {path} answers as {}: {}",
        answer.id, resp.2
    );
    assert_error(
        resp,
        answer.status,
        answer.code,
        &answer.message,
        &[(answer.field.as_str(), answer.message.as_str())],
    );
}

/// Asserts that `token` is a version token: 16 lower-case hex digits.
fn assert_token(token: &str, path: &str) {
    assert!(
        token.len() == VERSION_LEN
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "GET {path} answered the version {token:?}, not {VERSION_LEN} lower-case hex digits"
    );
}

/// Asserts that `body`, the answer to `GET path`, holds exactly the `shown`
/// fields, a version token and the information line of a found card, and
/// returns the token. No key of the body names the verification code.
///
/// The information line is the catalog text the rule returns, which
/// [`found_card_info`] holds to the text [`SCREEN_VARS_ROW`] quotes before
/// any request is sent.
fn assert_card(body: &Value, shown: &[(&'static str, String)], path: &str) -> String {
    let object = body
        .as_object()
        .unwrap_or_else(|| panic!("GET {path} answered no object: {body}"));
    let version = object
        .get(VERSION_KEY)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GET {path} answered no string {VERSION_KEY}: {body}"));
    assert_token(version, path);
    let mut expected: Map<String, Value> = shown
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Value::String(value.clone())))
        .collect();
    expected.insert(VERSION_KEY.to_owned(), Value::String(version.to_owned()));
    expected.insert(
        MESSAGE_KEY.to_owned(),
        Value::String(messages::DISPLAYING_REQUESTED_DETAILS.to_owned()),
    );
    assert_eq!(
        *object, expected,
        "GET {path} shows the card's screen fields, its token and the found card's information line, nothing else"
    );
    assert!(
        object.keys().all(|key| !key.contains("cvv")),
        "GET {path} names the verification code: {body}"
    );
    version.to_owned()
}

/// The view without a bearer token is refused before any edit.
async fn unauthenticated(app: &TestApp) {
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let resp = app.get(&path, None).await;
    assert_error(
        &resp,
        StatusCode::UNAUTHORIZED,
        UNAUTHENTICATED,
        messages::AUTHENTICATION_REQUIRED,
        &[],
    );
}

/// V-COCRDSLC-001 to V-COCRDSLC-006 — P-COCRDSLC-002 to P-COCRDSLC-004,
/// P-COCRDSLC-011, P-COCRDSLC-013 to P-COCRDSLC-015
///
/// Sends every case of [`CASES`] and returns the rows they exercised. Each
/// case's answer must be the [`winner`] of the rows it fires, so the table
/// follows the audit's order column; the cases pairing a rejected account
/// with the missing card show that a key error skips the read.
async fn edit_and_not_found_rows(
    app: &TestApp,
    token: &str,
    answers: &Answers,
) -> BTreeSet<&'static str> {
    let mut exercised = BTreeSet::new();
    for case in &CASES {
        assert_eq!(
            case.answer,
            winner(case.fired, answers),
            "the case {:?} {:?} expects another row than the audit's precedence gives",
            case.card,
            case.account
        );
        let path = view_path(case.card, case.account);
        let resp = app.get(&path, Some(token)).await;
        assert_answer(&resp, answer_of(answers, case.answer), &path);
        exercised.insert(case.answer);
    }
    exercised
}

/// An account number longer than `ACCTSIDI X(11)` has no 3270 counterpart:
/// it is refused before the edits, with no message, at `ACCTSID`.
async fn over_width_account(app: &TestApp, token: &str) {
    let path = view_path(EXAMPLE_CARD, Some(OVER_WIDTH_ACCOUNT));
    let field = CardViewField::AcctSid.as_str();
    assert_error(
        &app.get(&path, Some(token)).await,
        StatusCode::BAD_REQUEST,
        ErrorKind::Invalid.code(),
        NO_MESSAGE,
        &[(field, NO_MESSAGE)],
    );
}

/// P-COCRDSLC-006, P-CVACT02Y-001 to P-CVACT02Y-006, P-CVACT02Y-008
///
/// Every fixture card, card `9680294154603697` of account `00000000001`
/// [app/data/ASCII/carddata.txt:L49] among them, read with its own account,
/// shows exactly its record's screen fields, a version token and the found
/// card's information line, and no response value is its verification code.
async fn every_fixture_card(app: &TestApp, token: &str, cards: &[FixtureCard]) {
    for card in cards {
        let path = view_path(card.value("card_number"), Some(card.value("account_id")));
        let (status, body, raw) = app.get(&path, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
        assert_card(&body, &card.shown, &path);
        assert!(
            body.as_object().is_some_and(|object| object
                .values()
                .all(|value| value.as_str() != Some(card.cvv.as_str()))),
            "GET {path} carries the verification code: {raw}"
        );
    }
}

/// P-CVACT02Y-009, P-COCRDSLC-016 (E-55)
///
/// The seeded example card shows the section D values, and a second read
/// returns the same body and token. Entered with another account, the card is
/// still read by its number alone and shown with its own account and the same
/// token: the account entered is edited but never compared.
async fn example_card_reads(app: &TestApp, token: &str, example: &[(&'static str, String)]) {
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, first, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    let version = assert_card(&first, example, &path);

    let (status, again, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path} again: {raw}");
    assert_eq!(
        assert_card(&again, example, &path),
        version,
        "the token of an unchanged card is stable"
    );
    assert_eq!(
        again, first,
        "a second read of {path} answers the same body"
    );

    let other = view_path(EXAMPLE_CARD, Some(OTHER_ACCOUNT));
    let (status, body, raw) = app.get(&other, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {other}: {raw}");
    assert_eq!(
        body, first,
        "GET {other} reads card {EXAMPLE_CARD} by its number alone"
    );
}

/// COCRDSLC 1200-SETUP-SCREEN-VARS — P-COCRDSLC-010
///
/// The read sets `FOUND-CARDS-FOR-ACCOUNT` only for a card it finds, so only
/// that answer carries the information line: `info`, the text
/// [`SCREEN_VARS_ROW`] quotes, byte for byte with its three leading spaces,
/// beside the card whose number was entered. Every other send moves
/// `WS-RETURN-MSG` to `ERRMSGO`: each case of [`CASES`] answers its row's
/// error text and never the information line, and a key its edit refused is
/// zeroed, so the answer does not echo it.
async fn information_line(app: &TestApp, token: &str, answers: &Answers, info: &str) {
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, body, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    assert_eq!(
        (body[MESSAGE_KEY].as_str(), body["card_number"].as_str()),
        (Some(info), Some(EXAMPLE_CARD)),
        "P-COCRDSLC-010: GET {path} shows the found card's information line with the card entered: {raw}"
    );
    let shown = info.trim();
    for case in &CASES {
        let path = view_path(case.card, case.account);
        let resp = app.get(&path, Some(token)).await;
        assert_answer(&resp, answer_of(answers, case.answer), &path);
        assert!(
            !resp.2.contains(shown),
            "P-COCRDSLC-010: GET {path} shows the information line without a card: {}",
            resp.2
        );
        for (key, rows) in [
            (case.account, [V001, V002]),
            (Some(case.card), [V003, V004]),
        ] {
            let refused = rows.iter().any(|row| case.fired.contains(row));
            if let Some(key) = key.filter(|key| refused && !key.is_empty() && !key.contains('%')) {
                assert!(
                    !resp.2.contains(key),
                    "P-COCRDSLC-010: GET {path} echoes the refused key {key:?}: {}",
                    resp.2
                );
            }
        }
    }
}

/// V-COCRDSLC-007 — P-COCRDSLC-005, P-COCRDSLC-013
///
/// With the card table hidden, the read answers the `CARDDAT` file-error text
/// at `ACCTSID`, while a request whose key edit fails still answers its edit
/// text: the key error skips the read, which would otherwise move the
/// file-error text unconditionally. The table is then put back and the card
/// reads as before.
async fn read_failure_row(app: &TestApp, token: &str, answers: &Answers) {
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, before, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path} before the fault: {raw}");
    app.stack().exec_sql(HIDE_CARDS).await;

    let failed = app.get(&path, Some(token)).await;
    assert_answer(&failed, answer_of(answers, V007), &path);
    let blank = view_path(EXAMPLE_CARD, None);
    let refused = app.get(&blank, Some(token)).await;
    assert_answer(&refused, answer_of(answers, V001), &blank);

    app.stack().exec_sql(RESTORE_CARDS).await;
    let (status, after, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path} after the fault: {raw}");
    assert_eq!(after, before, "the card reads as before the fault");
}

/// The `OpenAPI` document the router publishes.
fn openapi() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The text of the first code span of the `Expected result` cell of the
/// parity row `id`, without its double quotes.
fn quoted_result(doc: &Document, id: &str) -> String {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let cell = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    let span = code_span(cell).unwrap_or_else(|| panic!("{id} quotes no text: {cell}"));
    unquoted(&span.text)
}

/// The information line [`SCREEN_VARS_ROW`] gives a found card: the text of
/// the `info=` that follows [`FOUND_CARD_FLAG`], without its double quotes,
/// which must equal the catalog text the rule returns byte for byte.
fn found_card_info(doc: &Document) -> String {
    let (table, row) = doc
        .find_row(SCREEN_VARS_ROW)
        .unwrap_or_else(|| panic!("the parity audit has no {SCREEN_VARS_ROW} row"));
    let cell = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{SCREEN_VARS_ROW} has no `Expected result` cell"));
    let Some(pair) = key_values(cell)
        .into_iter()
        .find(|pair| pair.key.contains(FOUND_CARD_FLAG) && pair.key.ends_with(INFO_KEY))
    else {
        panic!("{SCREEN_VARS_ROW} gives no {INFO_KEY}= after {FOUND_CARD_FLAG}: {cell}");
    };
    let span = code_span(&pair.value).unwrap_or_else(|| {
        panic!(
            "{SCREEN_VARS_ROW} quotes no {INFO_KEY} text: {}",
            pair.value
        )
    });
    let info = unquoted(&span.text);
    assert_eq!(
        info,
        messages::DISPLAYING_REQUESTED_DETAILS,
        "{SCREEN_VARS_ROW}: the catalog text differs from the audited information line"
    );
    info
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

/// The `VALUE` literal of the field `name` of the copybook `file`: the text
/// between the outer single quotes of the first line, from the field's own
/// line on, that is no comment (`*` in column 7) and holds a quote.
fn copybook_value(file: &str, name: &str) -> String {
    let path = fixtures::repo_root().join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    text.lines()
        .skip_while(|line| !line.split_whitespace().any(|word| word == name))
        .find(|line| line.as_bytes().get(COMMENT_COLUMN) != Some(&b'*') && line.contains('\''))
        .and_then(|line| {
            let start = line.find('\'')?;
            let end = line.rfind('\'')?;
            line.get(start + 1..end)
        })
        .unwrap_or_else(|| panic!("{file} gives {name} no VALUE literal"))
        .to_owned()
}

/// The `route` of the entry of [`MENU_FILE`] whose `program` is `program`.
fn menu_route(program: &str) -> String {
    let path = fixtures::repo_root().join(MENU_FILE);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let entry = format!("program: \"{program}\",");
    text.split_once(&entry)
        .and_then(|(_, rest)| rest.split_once("route: \""))
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or_else(
            || panic!("{MENU_FILE} has no menu option of {program} with a route"),
            |(route, _)| route.to_owned(),
        )
}

/// Every string of the JSON value `value`, at any depth.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        Value::Object(map) => map.values().flat_map(strings).collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => Vec::new(),
    }
}

/// Whether `value` has the shape of the header's `MM/DD/YY` date or
/// `HH:MM:SS` time: three pairs of digits, separated by `/` or `:`.
fn clock_shaped(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 8
        && [b'/', b':'].into_iter().any(|separator| {
            bytes.iter().enumerate().all(|(index, &byte)| {
                if index % 3 == 2 {
                    byte == separator
                } else {
                    byte.is_ascii_digit()
                }
            })
        })
}

/// Sends `GET path` with `token` and `Accept: accept`, and returns the
/// answer's `Content-Type` and the answer.
async fn get_as(app: &TestApp, path: &str, token: &str, accept: &str) -> (String, Resp) {
    let response = app
        .client()
        .get(app.url(path))
        .bearer_auth(token)
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
    (content_type, (status, body, raw))
}

/// COCRDSLC 0000-MAIN — P-COCRDSLC-001 (E-56)
///
/// No AID key reaches `GET /cards/{id}`: the `OpenAPI` document declares the
/// card number of the path and the `account_id` query parameter only, and an
/// [`AID_PARAMETER`] sent anyway is not read. Every key `YYYY-STORE-PFKEY`
/// stores therefore answers what ENTER answers, never the invalid-key text:
/// the card with the example keys, and V-COCRDSLC-001 with no account.
async fn aid_keys_act_as_enter(app: &TestApp, token: &str, answers: &Answers) {
    let doc = openapi();
    let declared: BTreeSet<(&str, &str)> = doc["paths"][VIEW_PATH]["get"]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("the OpenAPI document declares no parameters of {VIEW_PATH}"))
        .iter()
        .map(|parameter| {
            (
                parameter["name"].as_str().unwrap_or_default(),
                parameter["in"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        declared,
        BTreeSet::from(VIEW_PARAMETERS),
        "P-COCRDSLC-001: GET {VIEW_PATH} takes the two keys and no AID key"
    );
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, enter, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    let blank = view_path(EXAMPLE_CARD, None);
    let invalid_key = messages::CCDA_MSG_INVALID_KEY.trim_end();
    for key in aid_keys() {
        let keyed = format!("{path}&{AID_PARAMETER}={key}");
        let (status, body, raw) = app.get(&keyed, Some(token)).await;
        assert_eq!(
            (status, &body),
            (StatusCode::OK, &enter),
            "P-COCRDSLC-001: GET {keyed} answers as ENTER does: {raw}"
        );
        let keyed = format!("{blank}?{AID_PARAMETER}={key}");
        let resp = app.get(&keyed, Some(token)).await;
        assert_answer(&resp, answer_of(answers, V001), &keyed);
        assert!(
            !resp.2.contains(invalid_key),
            "P-COCRDSLC-001: GET {keyed} shows the invalid-key text: {}",
            resp.2
        );
    }
}

/// COCRDSLC COMMON-RETURN — P-COCRDSLC-007
///
/// The `RETURN TRANSID COMMAREA` of `COMMON-RETURN` becomes a stateless
/// `GET /cards/{id}?account_id=`: no request leaves state for the next. The
/// example card answers its first body again after a request with both keys
/// blank and after the view of another card, to the same user and to
/// another, and no answer sets a cookie.
async fn no_conversation_state(app: &TestApp, token: &str, answers: &Answers) {
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, first, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    let blank = view_path("%20", None);
    assert_answer(
        &app.get(&blank, Some(token)).await,
        answer_of(answers, V005),
        &blank,
    );
    let other = view_path(OTHER_CARD, Some(OTHER_ACCOUNT));
    let (status, _, raw) = app.get(&other, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {other}: {raw}");
    let admin = app.admin_token().await;
    for (who, bearer) in [("the same user", token), ("another user", admin.as_str())] {
        let (status, again, raw) = app.get(&path, Some(bearer)).await;
        assert_eq!(
            (status, &again),
            (StatusCode::OK, &first),
            "P-COCRDSLC-007: GET {path} by {who}, after other requests, answers as at first: {raw}"
        );
    }
    let response = app
        .client()
        .get(app.url(&path))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET {path} failed: {error}"));
    assert!(
        response.headers().get(SET_COOKIE).is_none(),
        "P-COCRDSLC-007: GET {path} keeps conversation state in a cookie: {:?}",
        response.headers()
    );
}

/// COCRDSLC 1000-SEND-MAP, 0000-MAIN after the EVALUATE — P-COCRDSLC-008,
/// P-COCRDSLC-024
///
/// The map is sent after the key edits, and the card is read only when they
/// pass: with the card master hidden, every case of [`CASES`] whose keys
/// fail answers its edit text, which needs no read, and only the case whose
/// keys pass reaches the read, which then fails (V-COCRDSLC-007). Each
/// answer is the one `WHEN CDEMO-PGM-REENTER` sends, complete at its one
/// field, so the `IF INPUT-ERROR` after the EVALUATE, not carried, has
/// nothing left to send. A first entry from the menu reads no card either:
/// the menu option of `COCRDSLC` opens the card list, where the keys are
/// chosen.
async fn edits_gate_the_read(app: &TestApp, token: &str, answers: &Answers) {
    app.stack().exec_sql(HIDE_CARDS).await;
    for case in &CASES {
        let expected = if case.answer == V006 {
            V007
        } else {
            case.answer
        };
        let path = view_path(case.card, case.account);
        let resp = app.get(&path, Some(token)).await;
        assert_answer(&resp, answer_of(answers, expected), &path);
        assert_eq!(
            resp.1["fields"].as_array().map(Vec::len),
            Some(1),
            "P-COCRDSLC-024: GET {path} is answered once, at one field: {}",
            resp.2
        );
    }
    app.stack().exec_sql(RESTORE_CARDS).await;
    assert_eq!(
        menu_route(PROGRAM),
        CARDS,
        "P-COCRDSLC-008: the menu option of {PROGRAM} opens the card list, which reads no card"
    );
}

/// COCRDSLC 1100-SCREEN-INIT — P-COCRDSLC-009
///
/// No API field carries the 3270 header: the found card holds its screen
/// fields, token and information line only, and neither it nor an error body
/// holds a title of
/// `COTTL01Y`, the transaction or program name [`SCREEN_INIT_ROW`] gives, or
/// a value shaped as the header's `MM/DD/YY` date or `HH:MM:SS` time.
async fn no_screen_header(
    app: &TestApp,
    token: &str,
    parity: &Document,
    example: &[(&'static str, String)],
) {
    let literals = HEADER_LITERALS.map(|field| {
        result_value(parity, SCREEN_INIT_ROW, field)
            .trim_matches('\'')
            .to_owned()
    });
    let titles = HEADER_TITLES.map(|field| {
        let name = result_value(parity, SCREEN_INIT_ROW, field);
        copybook_value(TITLE_COPYBOOK, &name).trim().to_owned()
    });
    let header: Vec<&String> = literals.iter().chain(&titles).collect();
    assert!(
        header.iter().all(|text| !text.is_empty()),
        "{SCREEN_INIT_ROW} gives every header field a text: {header:?}"
    );
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, card, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    assert_card(&card, example, &path);
    let blank = view_path(EXAMPLE_CARD, None);
    let (_, error, _) = app.get(&blank, Some(token)).await;
    for (asked, body) in [(&path, &card), (&blank, &error)] {
        for value in strings(body) {
            assert!(
                header.iter().all(|text| !value.contains(text.as_str())),
                "P-COCRDSLC-009: GET {asked} carries the screen header in {value:?}"
            );
            assert!(
                !clock_shaped(value),
                "P-COCRDSLC-009: GET {asked} carries a header date or time: {value:?}"
            );
        }
    }
}

/// COCRDSLC 1400-SEND-SCREEN — P-COCRDSLC-012
///
/// The `SEND MAP` is the HTTP response: the `OpenAPI` document declares the
/// card as the 200 answer and the error body as every other, each as JSON
/// only, and the answers are JSON, the card and the error body of a card not
/// on file.
async fn send_is_the_response(
    app: &TestApp,
    token: &str,
    answers: &Answers,
    example: &[(&'static str, String)],
) {
    let doc = openapi();
    let responses = doc["paths"][VIEW_PATH]["get"]["responses"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document declares no answers of {VIEW_PATH}"));
    let declared: BTreeSet<&str> = responses.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = VIEW_ANSWERS.iter().map(|(status, _)| *status).collect();
    assert_eq!(
        declared, expected,
        "P-COCRDSLC-012: the answers of GET {VIEW_PATH}"
    );
    for (status, schema) in VIEW_ANSWERS {
        let content = responses[status]["content"]
            .as_object()
            .unwrap_or_else(|| panic!("GET {VIEW_PATH} declares {status} without content"));
        let reference = content
            .get(JSON)
            .and_then(|media| media["schema"]["$ref"].as_str())
            .unwrap_or_default();
        assert!(
            content.len() == 1 && reference.ends_with(&format!("/{schema}")),
            "P-COCRDSLC-012: GET {VIEW_PATH} answers {status} as the JSON {schema}: {content:?}"
        );
    }
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (content_type, (status, card, raw)) = get_as(app, &path, token, JSON).await;
    assert!(
        status == StatusCode::OK && content_type.starts_with(JSON),
        "P-COCRDSLC-012: GET {path} answers {status} as {content_type:?}: {raw}"
    );
    assert_card(&card, example, &path);
    let missing = view_path(MISSING_CARD, Some(EXAMPLE_ACCOUNT));
    let (content_type, resp) = get_as(app, &missing, token, JSON).await;
    assert!(
        content_type.starts_with(JSON),
        "P-COCRDSLC-012: GET {missing} answers {content_type:?}"
    );
    assert_answer(&resp, answer_of(answers, V006), &missing);
}

/// COCRDSLC SEND-LONG-TEXT, SEND-PLAIN-TEXT — P-COCRDSLC-018,
/// P-COCRDSLC-019, P-COCRDSLC-022 (E-67)
///
/// Neither text paragraph is carried. No request carries the program
/// context `WHEN OTHER` tests, so a context other than enter or re-enter
/// sent anyway is not read, and the card answers as without it. A request
/// that asks for plain text still gets JSON: the card, or the error body of
/// its edit. No answer is the text [`PLAIN_TEXT_ROW`] quotes.
async fn no_plain_text(
    app: &TestApp,
    token: &str,
    answers: &Answers,
    parity: &Document,
    example: &[(&'static str, String)],
) {
    let scenario = quoted_result(parity, PLAIN_TEXT_ROW);
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    for context in OTHER_CONTEXTS {
        let with = format!("{path}&{CONTEXT_PARAMETER}={context}");
        let (status, body, raw) = app.get(&with, Some(token)).await;
        assert_eq!(status, StatusCode::OK, "P-COCRDSLC-019: GET {with}: {raw}");
        assert_card(&body, example, &with);
        assert!(
            !raw.contains(&scenario),
            "P-COCRDSLC-019: GET {with} answers {scenario:?}: {raw}"
        );
    }
    let blank = view_path(EXAMPLE_CARD, None);
    for (asked, edit) in [(&path, None), (&blank, Some(V001))] {
        let (content_type, resp) = get_as(app, asked, token, PLAIN_TEXT).await;
        assert!(
            content_type.starts_with(JSON),
            "P-COCRDSLC-018, P-COCRDSLC-022: GET {asked} asking for {PLAIN_TEXT} answers {content_type:?}"
        );
        if let Some(id) = edit {
            assert_answer(&resp, answer_of(answers, id), asked);
        } else {
            assert_eq!(resp.0, StatusCode::OK, "GET {asked}: {}", resp.2);
            assert_card(&resp.1, example, asked);
        }
        assert!(
            !resp.2.contains(&scenario),
            "P-COCRDSLC-019: GET {asked} answers {scenario:?}: {}",
            resp.2
        );
    }
}

/// COCRDSLC 9150-GETCARD-BYACCT — P-COCRDSLC-017, P-COCRDSLC-021 (E-55)
///
/// The account search is never performed, and the card is read by its
/// number alone. An account no card belongs to, with no card number, gets
/// the card edit's text, never the not-found text [`ACCOUNT_SEARCH_ROW`]
/// quotes, and gets it with the card master hidden too, where a search of
/// the cards would fail; with a card number it gets that card, of its own
/// account.
async fn no_account_search(
    app: &TestApp,
    token: &str,
    answers: &Answers,
    parity: &Document,
    cards: &[FixtureCard],
    example: &[(&'static str, String)],
) {
    let account = result_value(parity, MISSING_ACCOUNT_ROW, "value");
    assert!(
        cards.iter().all(|card| card.value("account_id") != account),
        "{MISSING_ACCOUNT_ROW}: {CARD_FILE} holds no card of account {account}"
    );
    let not_found = quoted_result(parity, ACCOUNT_SEARCH_ROW);
    let no_card = view_path("%20%20%20", Some(account.as_str()));
    for hidden in [false, true] {
        if hidden {
            app.stack().exec_sql(HIDE_CARDS).await;
        }
        let resp = app.get(&no_card, Some(token)).await;
        assert_answer(&resp, answer_of(answers, V003), &no_card);
        assert!(
            !resp.2.contains(&not_found),
            "P-COCRDSLC-017, P-COCRDSLC-021: GET {no_card} searches the cards by account: {}",
            resp.2
        );
    }
    app.stack().exec_sql(RESTORE_CARDS).await;
    let path = view_path(EXAMPLE_CARD, Some(account.as_str()));
    let (status, body, raw) = app.get(&path, Some(token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "P-COCRDSLC-017: GET {path} reads the card by its number: {raw}"
    );
    assert_card(&body, example, &path);
}

/// COCRDSLC 1300-SETUP-SCREEN-ATTRS — P-COCRDSLC-025
///
/// The information line's attribute, like every 3270 attribute, has no API
/// counterpart: every error body of [`CASES`] holds its code, message and
/// fields only, each field entry its BMS field and text only, and the found
/// card holds its screen fields, token and information line only.
async fn no_screen_attributes(app: &TestApp, token: &str, example: &[(&'static str, String)]) {
    let error_keys = BTreeSet::from(ERROR_KEYS);
    let field_keys = BTreeSet::from(FIELD_KEYS);
    for case in &CASES {
        let path = view_path(case.card, case.account);
        let (_, body, raw) = app.get(&path, Some(token)).await;
        let keys: BTreeSet<&str> = body
            .as_object()
            .into_iter()
            .flat_map(|object| object.keys().map(String::as_str))
            .collect();
        assert_eq!(
            keys, error_keys,
            "P-COCRDSLC-025: GET {path} answers an error body only: {raw}"
        );
        for entry in body["fields"].as_array().into_iter().flatten() {
            let keys: BTreeSet<&str> = entry
                .as_object()
                .into_iter()
                .flat_map(|object| object.keys().map(String::as_str))
                .collect();
            assert_eq!(
                keys, field_keys,
                "P-COCRDSLC-025: GET {path} marks its field without an attribute: {raw}"
            );
        }
    }
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let (status, card, raw) = app.get(&path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    assert_card(&card, example, &path);
}

/// COCRDSLC ABEND-ROUTINE — P-COCRDSLC-020, P-COCRDSLC-023 (E-67)
///
/// An abend of the task is answered by the HTTP error mapping's 500: with
/// the database pool closed, the view over HTTP and the `get_card` handler
/// called directly, whose own unit of work then cannot begin, both answer
/// 'Unexpected error' with no field. Neither carries the text
/// [`ABEND_ROW`] quotes, which `ABEND-ROUTINE` never moves, nor the culprit
/// or a code of `ABEND-DATA`. The pool stays closed, so this runs last.
async fn unexpected_failure(app: &TestApp, token: &str, parity: &Document) {
    let abend = quoted_result(parity, ABEND_ROW);
    let db = Db::from_pool(app.stack().pool().clone());
    db.close().await;
    let path = view_path(EXAMPLE_CARD, Some(EXAMPLE_ACCOUNT));
    let over_http = app.get(&path, Some(token)).await;
    let settings = Settings {
        database_url: app.stack().database_url().to_owned(),
        ..Settings::default()
    };
    let state = AppState::new(db, settings)
        .unwrap_or_else(|error| panic!("the test settings were rejected: {error}"));
    let user = AuthUser {
        user_id: SIGNED_ON_USER.to_owned(),
        usr_type: UserType::User,
    };
    let query = CardViewQuery {
        account_id: Some(EXAMPLE_ACCOUNT.to_owned()),
    };
    let viewed = get_card(
        user,
        State(state),
        ApiPath(EXAMPLE_CARD.to_owned()),
        ApiQuery(query),
    )
    .await;
    let Err(error) = viewed else {
        panic!("P-COCRDSLC-020: get_card answered 2xx over a closed pool");
    };
    let body = serde_json::to_value(error.body())
        .unwrap_or_else(|failure| panic!("the error body does not serialize: {failure}"));
    let direct = (error.status(), body.clone(), body.to_string());
    for (how, resp) in [("over HTTP", &over_http), ("called directly", &direct)] {
        assert_error(
            resp,
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::Unexpected.code(),
            messages::UNEXPECTED_ERROR,
            &[],
        );
        let abend_data = [abend.as_str(), PROGRAM].into_iter().chain(ABEND_CODES);
        for text in abend_data {
            assert!(
                !resp.2.contains(text),
                "P-COCRDSLC-020, P-COCRDSLC-023: the view {how} answers {text:?}: {}",
                resp.2
            );
        }
    }
}

/// Every COCRDSLC path of the audits through `GET /cards/{id}?account_id=`,
/// on one seeded database: the edit table, the found cards, the dialogue,
/// the read failures, the stored cards unchanged by every view, and last the
/// abend. Every row this file relies on must have passed its review, and the
/// requests must exercise every COCRDSLC row the validation audit defines.
#[test]
fn card_view_answers_every_audited_cocrdslc_path() {
    let validation = validation_audit();
    let parity = parity_audit();
    for id in PARITY_ROWS.iter().chain(&DIALOGUE_ROWS) {
        assert_reviewed(&parity, id);
    }
    let answers = answers(&validation);
    let cards = fixture_cards(&parity);
    let example = example_card(&parity, &cards);
    let info = found_card_info(&parity);
    let app = TestApp::start();
    let exercised = block_on(async {
        let token = app.user_token().await;
        let stored = app.stack().fetch_text_rows(CARDS_SNAPSHOT).await;
        unauthenticated(&app).await;
        let mut exercised = edit_and_not_found_rows(&app, &token, &answers).await;
        over_width_account(&app, &token).await;
        every_fixture_card(&app, &token, &cards).await;
        example_card_reads(&app, &token, &example).await;
        information_line(&app, &token, &answers, &info).await;
        aid_keys_act_as_enter(&app, &token, &answers).await;
        no_conversation_state(&app, &token, &answers).await;
        no_screen_header(&app, &token, &parity, &example).await;
        send_is_the_response(&app, &token, &answers, &example).await;
        no_plain_text(&app, &token, &answers, &parity, &example).await;
        no_screen_attributes(&app, &token, &example).await;
        no_account_search(&app, &token, &answers, &parity, &cards, &example).await;
        edits_gate_the_read(&app, &token, &answers).await;
        read_failure_row(&app, &token, &answers).await;
        assert_eq!(
            app.stack().fetch_text_rows(CARDS_SNAPSHOT).await,
            stored,
            "P-COCRDSLC-008: every view leaves the stored cards as they were"
        );
        unexpected_failure(&app, &token, &parity).await;
        exercised.insert(V007);
        exercised
    });
    let defined: BTreeSet<&str> = answers.keys().map(String::as_str).collect();
    assert_eq!(
        exercised, defined,
        "the requests exercise exactly the {PROGRAM} rows the validation audit defines"
    );
}
