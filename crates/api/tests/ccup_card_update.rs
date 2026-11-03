//! `CCUP` (`COCRDUPC`, map `COCRDUP`) as `PUT /cards/{id}`, proved through the
//! HTTP layer against the `COCRDUPC` rows of `docs/parity/validation-audit.md`
//! and section B of `docs/parity/parity-oracle-audit.md`.
//!
//! One request runs the key edits, the fetch, the comparison with the fetched
//! card, the card-field edits, the locked read, the change check against the
//! `version` token and the rewrite (E-56). The tests cover:
//!
//! - every key and card-field edit, each with the status, `code`, message and
//!   BMS field of its audit row, and the first failing edit in COBOL `Order`
//!   supplying the answer when two edits fail (P-COCRDUPC-002);
//! - the missing `version` token, the no-message 400 of E-24;
//! - the no-change answer, which writes nothing;
//! - the rewrite, its success text and the record it stores;
//! - the change check: a concurrent change of case only passes it, any other
//!   concurrent change is the 409 'Record changed by some one else';
//! - the card file failures of the fetch, the lock and the rewrite;
//! - the 3270 mechanics the one request replaces: no AID key reaches the
//!   update and each map control is one request; the token, not a session,
//!   carries the fetched values; the body parses before the edits run, and
//!   the edits answer before any file access; every answer is one JSON body
//!   without screen header, map name or field attribute; the verification
//!   code is kept; and an unexpected failure is the 500 'Unexpected error',
//!   never the abend data.
//!
//! Each expected answer is read from its audit row through `carddemo_audit`,
//! and only from a row whose review verdict is `pass`; every message text is
//! also matched against its `carddemo_domain::messages` constant. Every V-ID
//! of `COCRDUPC` is exercised, which `every_cocrdupc_row_is_exercised` checks
//! without a container. The Docker-backed tests each start one seeded
//! `postgres:16.15` container; Docker must be running.

pub mod common;

use std::collections::BTreeSet;
use std::fs;

use carddemo_api::dto::cards::{CardUpdateField, CardUpdateRequest};
use carddemo_api::spec_json;
use carddemo_audit::{
    Document, Repo, cell_code_spans, citations, code_span, is_no_message, key_values,
    no_message_escalation,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use serde_json::{Map, Value};

/// The program whose validation rows this file exercises.
const PROGRAM: &str = "COCRDUPC";

/// The fixture row that pins the card every request addresses: the first
/// card of `carddata.txt`, the card the `COCRDUPC` examples of the
/// validation audit use.
///
/// The card is the audit-pinned one rather than any other fixture card,
/// because only this card's fields are pinned by a reviewed fixture row.
const FIXTURE_ROW: &str = "P-CVACT02Y-009";

/// The card number of [`FIXTURE_ROW`], checked against the row. The SQL
/// below names it as a literal, because SQL is never built with `format!`.
const FIXTURE_CARD: &str = "0500024453765740";

/// Every column of the fixture card's row: what a request that writes
/// nothing leaves unchanged, the verification code included.
const CARD_ROW_SQL: &str = "SELECT card_number, card_acct_id, card_cvv_cd, card_embossed_name,
        card_expiraion_date, card_active_status
   FROM cards
  WHERE card_number = '0500024453765740'";

/// The columns a rewrite is compared on: the account as its 11 digits, the
/// name, the expiry date and the status. The verification code is left out:
/// what an update stores there is unpinned (E-75, at `halt`), so it is
/// excluded from parity comparison until it is decided.
const STORED_CARD_SQL: &str = "SELECT lpad(card_acct_id::text, 11, '0'), card_embossed_name,
        card_expiraion_date, card_active_status
   FROM cards
  WHERE card_number = '0500024453765740'";

/// Another session changing only the case of the fixture card's name.
const UPPER_CASE_NAME_SQL: &str = "UPDATE cards SET card_embossed_name = upper(card_embossed_name)
  WHERE card_number = '0500024453765740'";

/// Another session renaming the fixture card.
const RENAME_ELSEWHERE_SQL: &str = "UPDATE cards SET card_embossed_name = 'SOMEONE ELSE'
  WHERE card_number = '0500024453765740'";

/// The statements that name the fixture card.
const FIXTURE_SQL: [&str; 4] = [
    CARD_ROW_SQL,
    STORED_CARD_SQL,
    UPPER_CASE_NAME_SQL,
    RENAME_ELSEWHERE_SQL,
];

/// The fixture row of the card number no card has.
const MISSING_ROW: &str = "P-CVACT02Y-010";

/// The card number of [`MISSING_ROW`], checked against the row.
const MISSING_CARD: &str = "9999999999999999";

/// The parity row whose `info` is the text of a committed rewrite.
const COMMITTED_ROW: &str = "P-COCRDUPC-007";

/// The no-change row, a 200 that writes nothing.
const NO_CHANGE_ROW: &str = "V-COCRDUPC-009";

/// The concurrent-change row, the 409 of `9300-CHECK-CHANGE-IN-REC`.
const CONFLICT_ROW: &str = "V-COCRDUPC-019";

/// The rows the outcome test exercises, outside the edit table.
const OUTCOME_ROWS: [&str; 2] = [NO_CHANGE_ROW, CONFLICT_ROW];

/// The escalation that decides the missing `version` token.
const VERSION_ESCALATION: &str = "E-24";

/// The `message` of a row that sets none: E-24 names no text, so the body
/// carries the empty text, as for every `no-message` row whose escalation
/// names none.
const NO_MESSAGE: &str = "";

/// The `BMS field` cell of a row that names no field.
const NO_BMS_FIELD: &str = "—";

/// The `kind` of a row that is no error: a message-only 200.
const NO_KIND: &str = "none";

/// The `RESP` E-44 proposes for a database failure: `IOERR`.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 proposes alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// A changed embossed name: letters and spaces only, in mixed case, so the
/// stored text shows the name is kept as entered (P-COCRDUPC-013).
const CHANGED_NAME: &str = "Aniya Q Von";

/// An account number other than the fixture card's: the card is read by its
/// number alone and the account is not compared (E-55).
const OTHER_ACCOUNT: &str = "00000000001";

/// The expiry month and year of the re-dated card, both within their ranges.
const NEW_EXPIRY: (&str, &str) = ("11", "2024");

/// The status of the re-dated card.
const NEW_STATUS: &str = "N";

/// `0000-MAIN`: every key but ENTER, PF3, PF5 after the validation and PF12
/// after a fetch acts as ENTER.
const ANY_KEY_ROW: &str = "P-COCRDUPC-001";

/// `2000-DECIDE-ACTION`: edits that pass prompt for PF5, whose `info` text
/// this row quotes, and only PF5 writes.
const VALIDATED_ROW: &str = "P-COCRDUPC-003";

/// `COMMON-RETURN`: the update state the COMMAREA keeps between turns.
const COMMAREA_ROW: &str = "P-COCRDUPC-008";

/// `1000-PROCESS-INPUTS`: the map is received, then edited.
const PROCESS_INPUTS_ROW: &str = "P-COCRDUPC-009";

/// `3000-SEND-MAP`: every turn ends with the screen.
const SEND_MAP_ROW: &str = "P-COCRDUPC-017";

/// `3100-SCREEN-INIT`: the screen header, whose transaction and program
/// names this row quotes.
const SCREEN_INIT_ROW: &str = "P-COCRDUPC-018";

/// `3200-SETUP-SCREEN-VARS`: the values the screen shows.
const SCREEN_VARS_ROW: &str = "P-COCRDUPC-019";

/// `3300-SETUP-SCREEN-ATTRS`: the protection, cursor and colour of the
/// fields.
const SCREEN_ATTRS_ROW: &str = "P-COCRDUPC-020";

/// `3400-SEND-SCREEN`: the `SEND MAP`, whose mapset and map names this row
/// quotes in its `action`.
const SEND_SCREEN_ROW: &str = "P-COCRDUPC-021";

/// `YYYY-STORE-PFKEY`: the AID key the task stores, whose `CCARD-AID` values
/// this row cites.
const STORE_PFKEY_ROW: &str = "P-COCRDUPC-024";

/// `ABEND-ROUTINE`: what an abend of the task sends, and the text that
/// replaces it.
const ABEND_ROW: &str = "P-COCRDUPC-025";

/// `ABEND-ROUTINE`: the abend message that is never moved (E-67).
const ABEND_MSG_ROW: &str = "P-COCRDUPC-026";

/// `2000-DECIDE-ACTION` `WHEN OTHER`: the abend no state reaches (E-67).
const DECIDE_OTHER_ROW: &str = "P-COCRDUPC-027";

/// `9200-WRITE-PROCESSING`: the verification code an update writes (E-75).
const CVV_ROW: &str = "P-COCRDUPC-028";

/// The parity rows these tests prove, each with a review verdict to check.
const PARITY_ROWS: [&str; 30] = [
    "P-CVACT02Y-009",
    "P-CVACT02Y-010",
    ANY_KEY_ROW,
    "P-COCRDUPC-002",
    VALIDATED_ROW,
    "P-COCRDUPC-004",
    "P-COCRDUPC-005",
    "P-COCRDUPC-006",
    "P-COCRDUPC-007",
    COMMAREA_ROW,
    PROCESS_INPUTS_ROW,
    "P-COCRDUPC-010",
    "P-COCRDUPC-011",
    "P-COCRDUPC-012",
    "P-COCRDUPC-013",
    "P-COCRDUPC-014",
    "P-COCRDUPC-015",
    "P-COCRDUPC-016",
    SEND_MAP_ROW,
    SCREEN_INIT_ROW,
    SCREEN_VARS_ROW,
    SCREEN_ATTRS_ROW,
    SEND_SCREEN_ROW,
    "P-COCRDUPC-022",
    "P-COCRDUPC-023",
    STORE_PFKEY_ROW,
    ABEND_ROW,
    ABEND_MSG_ROW,
    DECIDE_OTHER_ROW,
    CVV_ROW,
];

/// The endpoint of the update, as the function map names it.
const UPDATE_ENDPOINT: &str = "PUT /cards/{id}";

/// The body keys of `PUT /cards/{id}`, in name order: the editable `COCRDUP`
/// map fields and the token. The card number is the path segment.
const UPDATE_BODY_KEYS: [&str; 6] = [
    "account_id",
    "active_status",
    "embossed_name",
    "expiry_month",
    "expiry_year",
    "version",
];

/// The keys of a 200 answer: the card and the information text.
const UPDATED_KEYS: [&str; 2] = ["card", "message"];

/// The keys of the card a 200 answer shows, in name order.
const CARD_KEYS: [&str; 6] = [
    "account_id",
    "card_active_status",
    "card_embossed_name",
    "card_expiraion_date",
    "card_number",
    "version",
];

/// The keys an error body may hold; `fields` is left out when empty.
const ERROR_KEYS: [&str; 3] = ["code", "fields", "message"];

/// The keys of one `fields` entry of an error body.
const FIELD_ENTRY_KEYS: [&str; 2] = ["field", "message"];

/// The media type every answer is sent as.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The `BMS map` cell of the update screen's function-map rows.
const MAPSET: &str = "COCRDUP";

/// The `UI control` or `Endpoint` cell of a function-map row with no
/// control or no request; a navigation row adds a note after it.
const NONE_CELL: &str = "—";

/// The `Key or field` of the function-map row for every other AID key.
const OTHER_KEYS: &str = "other AID keys";

/// The `Key or field` of the function-map rows for ENTER.
const ENTER_KEY: &str = "ENTER";

/// The `Key or field` of the function-map row for the PF5 save.
const SAVE_KEY: &str = "PF5";

/// The copybook whose `CCARD-AID` 88-level values [`STORE_PFKEY_ROW`] cites.
const AID_COPYBOOK: &str = "CVCRD01Y.cpy";

/// What precedes each `CCARD-AID` value in its 88-level line.
const AID_VALUE: &str = "VALUE '";

/// The `CCARD-AID` values: ENTER, CLEAR, PA1, PA2 and PF1 to PF12.
const AID_COUNT: usize = 16;

/// The body and query key a request names an AID key in, after `CCARD-AID`.
const AID_KEY: &str = "ccard_aid";

/// A body key naming a verification code, which the map has no field for.
const CVV_KEY: &str = "card_cvv_cd";

/// The verification code that key carries, other than the fixture card's.
const PROBE_CVV: &str = "999";

/// The column of the verification code in the rows of [`CARD_ROW_SQL`].
const CVV_COLUMN: usize = 2;

/// The name edit row whose input every key-carrying request repeats.
const NAME_PROBE: &str = "V-COCRDUPC-011";

/// The key edit row whose input reaches no file.
const KEY_PROBE: &str = "V-COCRDUPC-003";

/// The fault row of the card read, whose break hides the card master.
const FETCH_FAULT: &str = "V-COCRDUPC-008";

/// A JSON number where the map field is text: a body that does not parse.
const UNPARSED_NAME: u8 = 5;

/// The body field that does not parse, named by the refusal.
const UNPARSED_FIELD: &str = "embossed_name";

/// The text of the refused commit, which no answer may carry.
const REFUSAL_TEXT: &str = "card commit refused by the test";

/// Refuses every commit that updated a card, at `COMMIT`, after the rewrite
/// has run: a failure of `PUT /cards/{id}` no rule chose a text for.
const REFUSE_COMMIT_SQL: &str =
    "CREATE FUNCTION ccup_refuse_commit() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN RAISE EXCEPTION 'card commit refused by the test'; END $$;
                 CREATE CONSTRAINT TRIGGER ccup_refuse_commit AFTER UPDATE ON cards
                     DEFERRABLE INITIALLY DEFERRED
                     FOR EACH ROW EXECUTE FUNCTION ccup_refuse_commit()";

/// Mends what [`REFUSE_COMMIT_SQL`] breaks.
const ACCEPT_COMMIT_SQL: &str = "DROP TRIGGER ccup_refuse_commit ON cards;
                  DROP FUNCTION ccup_refuse_commit()";

/// The BMS fields a `PUT /cards/{id}` error may name, as the API publishes
/// them.
const UPDATE_FIELDS: [CardUpdateField; 6] = [
    CardUpdateField::AcctSid,
    CardUpdateField::CardSid,
    CardUpdateField::CrdName,
    CardUpdateField::CrdStCd,
    CardUpdateField::ExpMon,
    CardUpdateField::ExpYear,
];

/// The error kinds, whose `Debug` names are the audit's `kind=` values.
const KINDS: [ErrorKind; 7] = [
    ErrorKind::Invalid,
    ErrorKind::Empty,
    ErrorKind::NotFound,
    ErrorKind::Duplicate,
    ErrorKind::Conflict,
    ErrorKind::SignonFailed,
    ErrorKind::Unexpected,
];

/// The answer one validation audit row gives.
#[derive(Debug, Clone)]
struct Answer {
    /// The row ID, for failure texts.
    id: String,
    /// The `Status` column.
    status: StatusCode,
    /// The `code` of the row's `kind`, `None` for a message-only 200.
    code: Option<&'static str>,
    /// The `Message text`: the quoted literal, or empty for `no-message`.
    message: String,
    /// The `BMS field`, `None` for `—`.
    field: Option<String>,
    /// The `Order` column.
    order: u16,
}

/// One `PUT /cards/{id}` request: the `{id}` path segment and the body.
#[derive(Debug, Clone)]
struct Put {
    /// The `{id}` path segment, percent-encoded where it holds a space.
    card: String,
    /// The body, as the DTO reads it.
    body: CardUpdateRequest,
}

/// The fixture card as [`FIXTURE_ROW`] pins it.
#[derive(Debug, Clone)]
struct FixtureCard {
    /// `CARD-NUM`.
    card: String,
    /// `CARD-ACCT-ID` as its 11 digits.
    account: String,
    /// `CARD-EMBOSSED-NAME`, right-trimmed.
    name: String,
    /// `CARD-EXPIRAION-DATE`, `YYYY-MM-DD`.
    expiry: String,
    /// `CARD-ACTIVE-STATUS`.
    status: String,
}

/// One failing input of a `COCRDUPC` validation row.
struct Case {
    /// The `V-COCRDUPC-nnn` row the input triggers.
    id: &'static str,
    /// The catalog text the row's message equals.
    message: &'static str,
    /// The input: the unchanged request with this change applied.
    change: fn(&mut Put),
}

/// Enters `value` into the body field `field`.
fn enter(field: &mut Option<String>, value: &str) {
    *field = Some(value.to_owned());
}

/// Every input of the edit test, keyed by the validation row it triggers, in
/// the audit's order. A row has more than one input where the audit names
/// several.
///
/// `1100-RECEIVE-MAP` reads an absent, empty, blank or `*` field as not
/// entered (P-COCRDUPC-010). `FUNCTION UPPER-CASE` makes a lower-case `y`
/// with every other field unchanged a no-change answer, so the `y` input of
/// V-COCRDUPC-013 changes the name as well (P-COCRDUPC-014).
const EDIT_CASES: [Case; 25] = [
    Case {
        id: "V-COCRDUPC-001",
        message: NO_MESSAGE,
        change: |put| put.body.version = None,
    },
    Case {
        id: "V-COCRDUPC-002",
        message: messages::ACCOUNT_NUMBER_NOT_PROVIDED,
        change: |put| put.body.account_id = None,
    },
    Case {
        id: "V-COCRDUPC-002",
        message: messages::ACCOUNT_NUMBER_NOT_PROVIDED,
        change: |put| enter(&mut put.body.account_id, ""),
    },
    Case {
        id: "V-COCRDUPC-002",
        message: messages::ACCOUNT_NUMBER_NOT_PROVIDED,
        change: |put| enter(&mut put.body.account_id, "00000000000"),
    },
    Case {
        id: "V-COCRDUPC-003",
        message: messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT,
        change: |put| enter(&mut put.body.account_id, "0000000005A"),
    },
    Case {
        id: "V-COCRDUPC-004",
        message: messages::CARD_NUMBER_NOT_PROVIDED,
        change: |put| "0000000000000000".clone_into(&mut put.card),
    },
    Case {
        id: "V-COCRDUPC-005",
        message: messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
        change: |put| "050002445376574X".clone_into(&mut put.card),
    },
    Case {
        id: "V-COCRDUPC-006",
        message: messages::NO_INPUT_RECEIVED,
        change: |put| {
            "%20".clone_into(&mut put.card);
            enter(&mut put.body.account_id, "");
        },
    },
    Case {
        id: "V-COCRDUPC-007",
        message: messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION,
        change: |put| MISSING_CARD.clone_into(&mut put.card),
    },
    Case {
        id: "V-COCRDUPC-010",
        message: messages::CARD_NAME_NOT_PROVIDED,
        change: |put| put.body.embossed_name = None,
    },
    Case {
        id: "V-COCRDUPC-010",
        message: messages::CARD_NAME_NOT_PROVIDED,
        change: |put| enter(&mut put.body.embossed_name, ""),
    },
    Case {
        id: "V-COCRDUPC-010",
        message: messages::CARD_NAME_NOT_PROVIDED,
        change: |put| enter(&mut put.body.embossed_name, "   "),
    },
    Case {
        id: "V-COCRDUPC-010",
        message: messages::CARD_NAME_NOT_PROVIDED,
        change: |put| enter(&mut put.body.embossed_name, "*"),
    },
    Case {
        id: "V-COCRDUPC-011",
        message: messages::CARD_NAME_CAN_ONLY_CONTAIN_ALPHABETS_AND_SPACES,
        change: |put| enter(&mut put.body.embossed_name, "Aniya V0n"),
    },
    Case {
        id: "V-COCRDUPC-012",
        message: messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N,
        change: |put| enter(&mut put.body.active_status, ""),
    },
    Case {
        id: "V-COCRDUPC-012",
        message: messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N,
        change: |put| enter(&mut put.body.active_status, "0"),
    },
    Case {
        id: "V-COCRDUPC-013",
        message: messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N,
        change: |put| enter(&mut put.body.active_status, "X"),
    },
    Case {
        id: "V-COCRDUPC-013",
        message: messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N,
        change: |put| {
            enter(&mut put.body.embossed_name, CHANGED_NAME);
            enter(&mut put.body.active_status, "y");
        },
    },
    Case {
        id: "V-COCRDUPC-014",
        message: messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12,
        change: |put| enter(&mut put.body.expiry_month, ""),
    },
    Case {
        id: "V-COCRDUPC-014",
        message: messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12,
        change: |put| enter(&mut put.body.expiry_month, "00"),
    },
    Case {
        id: "V-COCRDUPC-015",
        message: messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12,
        change: |put| enter(&mut put.body.expiry_month, "13"),
    },
    Case {
        id: "V-COCRDUPC-016",
        message: messages::INVALID_CARD_EXPIRY_YEAR,
        change: |put| enter(&mut put.body.expiry_year, ""),
    },
    Case {
        id: "V-COCRDUPC-016",
        message: messages::INVALID_CARD_EXPIRY_YEAR,
        change: |put| enter(&mut put.body.expiry_year, "0000"),
    },
    Case {
        id: "V-COCRDUPC-017",
        message: messages::INVALID_CARD_EXPIRY_YEAR,
        change: |put| enter(&mut put.body.expiry_year, "1949"),
    },
    Case {
        id: "V-COCRDUPC-017",
        message: messages::INVALID_CARD_EXPIRY_YEAR,
        change: |put| enter(&mut put.body.expiry_year, "2100"),
    },
];

/// One representative failing input per edited value, combined pairwise to
/// prove that the first failing edit in COBOL `Order` answers
/// (P-COCRDUPC-002): the account, the card number, the name, the status,
/// the month and the year. Blank keys are left out, because two blank keys
/// answer with their own text (V-COCRDUPC-006).
const ORDER_PROBES: [&str; 6] = [
    "V-COCRDUPC-003",
    "V-COCRDUPC-005",
    "V-COCRDUPC-011",
    "V-COCRDUPC-013",
    "V-COCRDUPC-015",
    "V-COCRDUPC-017",
];

/// The text of a card file failure.
enum FaultText {
    /// A catalog constant.
    Catalog(&'static str),
    /// `WS-FILE-ERROR-MESSAGE`, composed from the `ERROR-OPNAME` and
    /// `ERROR-FILE` literals of the row's message cell and the E-44 codes.
    FileError,
}

/// One injected card file failure: the statements that break the card master
/// for one access and the statements that mend it.
struct Fault {
    /// The validation row the failure answers with.
    id: &'static str,
    /// The text the answer carries.
    text: FaultText,
    /// Breaks the access.
    inject: &'static str,
    /// Mends it.
    restore: &'static str,
}

/// The card file failures, one per access of `PUT /cards/{id}`.
///
/// - The fetch (`9100-GETCARD-BYACCTCARD` `WHEN OTHER`): the card master is
///   hidden, so the read fails on the server.
/// - The locked read (`9200-WRITE-PROCESSING` `READ … UPDATE`): the card
///   master is served through a `DISTINCT` view, which a plain read reads and
///   `FOR UPDATE` refuses, so only the lock fails.
/// - The rewrite (`REWRITE CARDDAT`): a trigger refuses every update of the
///   card master.
const FAULTS: [Fault; 3] = [
    Fault {
        id: "V-COCRDUPC-008",
        text: FaultText::FileError,
        inject: "ALTER TABLE cards RENAME TO cards_offline",
        restore: "ALTER TABLE cards_offline RENAME TO cards",
    },
    Fault {
        id: "V-COCRDUPC-018",
        text: FaultText::Catalog(messages::COULD_NOT_LOCK_RECORD_FOR_UPDATE),
        inject: "ALTER TABLE cards RENAME TO cards_base;
                 CREATE VIEW cards AS SELECT DISTINCT * FROM cards_base",
        restore: "DROP VIEW cards;
                  ALTER TABLE cards_base RENAME TO cards",
    },
    Fault {
        id: "V-COCRDUPC-020",
        text: FaultText::Catalog(messages::UPDATE_OF_RECORD_FAILED),
        inject: "CREATE FUNCTION ccup_refuse_rewrite() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN RAISE EXCEPTION 'card rewrite refused by the test'; END $$;
                 CREATE TRIGGER ccup_refuse_rewrite BEFORE UPDATE ON cards
                     FOR EACH ROW EXECUTE FUNCTION ccup_refuse_rewrite()",
        restore: "DROP TRIGGER ccup_refuse_rewrite ON cards;
                  DROP FUNCTION ccup_refuse_rewrite()",
    },
];

/// Asserts that a review log of `doc` records `id` and that its last verdict
/// is `pass`: only reviewed rows decide what these tests expect.
fn assert_reviewed(doc: &Document, id: &str) {
    let verdict = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| table.cell(row, "Row ID").map(str::trim) == Some(id))
        .filter_map(|(table, row)| table.cell(row, "Verdict"))
        .next_back();
    assert_eq!(
        verdict.map(str::trim),
        Some("pass"),
        "{id} must have passed its review before a test relies on it"
    );
}

/// The text a message cell's quoted literal holds: the code span without its
/// double quotes, `\"` read as `"` and `\\` as `\`.
fn unquote(span: &str) -> String {
    let inner = span
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("the code span {span} is no double-quoted literal"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            text.extend(chars.next());
        } else {
            text.push(ch);
        }
    }
    text
}

/// The `Expected result` cell of the parity row `id`, which must have passed
/// its review.
fn expected_cell<'a>(doc: &'a Document, id: &str) -> &'a str {
    assert_reviewed(doc, id);
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"))
}

/// The value of `key` in the `Expected result` cell of the parity row `id`,
/// as written, code spans included.
fn expected_entry(doc: &Document, id: &str, key: &str) -> String {
    let cell = expected_cell(doc, id);
    key_values(cell)
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| panic!("{id} pins no `{key}`: {cell}"))
        .value
}

/// The value of `key` in the `Expected result` cell of the parity row `id`:
/// the quoted literal of a code span, or the plain text.
fn expected_value(doc: &Document, id: &str, key: &str) -> String {
    let value = expected_entry(doc, id, key);
    code_span(&value).map_or(value, |span| unquote(&span.text))
}

/// The quoted literals of `text`, in order: the text of each code span, read
/// as [`unquote`] reads it.
///
/// # Panics
///
/// Panics, naming `id`, when `text` quotes none.
fn literals(text: &str, id: &str) -> Vec<String> {
    let found: Vec<String> = cell_code_spans(text)
        .iter()
        .map(|span| unquote(&span.text))
        .collect();
    assert!(!found.is_empty(), "{id} quotes no literal in: {text}");
    found
}

/// The `code` of the audit `kind` named `name`, `None` for [`NO_KIND`].
fn kind_code(name: &str, id: &str) -> Option<&'static str> {
    if name == NO_KIND {
        return None;
    }
    let kind = KINDS
        .into_iter()
        .find(|kind| format!("{kind:?}") == name)
        .unwrap_or_else(|| panic!("{id} names the unknown kind `{name}`"));
    Some(kind.code())
}

/// The answer of the validation row `id`, from its `Status`, `kind`,
/// `Message text`, `BMS field` and `Order` cells.
///
/// A `no-message` row must link E-24, the escalation of the missing token,
/// and answers with [`NO_MESSAGE`]. A named field must be one the API
/// publishes in [`CardUpdateField`].
fn answer(doc: &Document, id: &str) -> Answer {
    assert_reviewed(doc, id);
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
        .unwrap_or_else(|| panic!("{id} has no HTTP status: {}", cell("Status")));
    let kind = key_values(cell("Trigger condition"))
        .into_iter()
        .rfind(|pair| pair.key == "kind")
        .unwrap_or_else(|| panic!("{id} names no kind"))
        .value;
    let message_cell = cell("Message text");
    let message = if is_no_message(message_cell) {
        assert_eq!(
            no_message_escalation(message_cell).as_deref(),
            Some(VERSION_ESCALATION),
            "{id}: the only no-message path of {PROGRAM} is the missing version token"
        );
        NO_MESSAGE.to_owned()
    } else {
        let span = code_span(message_cell).unwrap_or_else(|| panic!("{id} quotes no text"));
        unquote(&span.text)
    };
    let field = Some(cell("BMS field"))
        .filter(|field| *field != NO_BMS_FIELD)
        .map(str::to_owned);
    if let Some(field) = &field {
        assert!(
            UPDATE_FIELDS.iter().any(|known| known.as_str() == field),
            "{id} names {field}, which `CardUpdateField` does not publish"
        );
    }
    Answer {
        id: id.to_owned(),
        status,
        code: kind_code(&kind, id),
        message,
        field,
        order: cell("Order")
            .parse()
            .unwrap_or_else(|_| panic!("{id} has no numeric Order")),
    }
}

/// The `ERROR-OPNAME` or `ERROR-FILE` literal `name` a message cell gives:
/// the single-quoted text after its last mention, for example `CARDDAT ` in
/// `ERROR-FILE 'CARDDAT '`.
fn quoted_literal<'a>(cell: &'a str, name: &str) -> &'a str {
    let (literal, _) = cell
        .rsplit_once(name)
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
        .unwrap_or_else(|| panic!("no quoted {name} in the message cell: {cell}"));
    literal
}

/// The answer of the fault row `fault.id`, with the text the fault gives.
fn fault_answer(doc: &Document, fault: &Fault) -> Answer {
    let mut expected = answer(doc, fault.id);
    match fault.text {
        FaultText::Catalog(text) => {
            assert_eq!(expected.message, text, "{}: catalog text", fault.id);
        }
        FaultText::FileError => {
            let (table, row) = doc
                .find_row(fault.id)
                .unwrap_or_else(|| panic!("the validation audit has no {} row", fault.id));
            let cell = table
                .cell(row, "Message text")
                .unwrap_or_else(|| panic!("{} has no `Message text` cell", fault.id));
            expected.message = messages::file_error_message(
                quoted_literal(cell, "ERROR-OPNAME"),
                quoted_literal(cell, "ERROR-FILE"),
                RESP_IOERR,
                RESP2_IOERR,
            );
        }
    }
    expected
}

/// Asserts that `resp` is the error answer `expected` gives: its status, the
/// `code` of its kind, its message, and its one BMS field carrying that
/// message, or no field for `—`.
///
/// The field is where `3300-SETUP-SCREEN-ATTRS` puts the cursor and the
/// colour, the BMS field the validation audit records for the text; the body
/// is the error body alone ([`assert_error_body`]).
fn assert_answer(resp: &Resp, expected: &Answer, context: &str) {
    let code = expected
        .code
        .unwrap_or_else(|| panic!("{context}: {} is no error row", expected.id));
    assert_eq!(
        resp.0, expected.status,
        "{context}: status of {}: {}",
        expected.id, resp.2
    );
    assert_eq!(
        resp.1["message"].as_str(),
        Some(expected.message.as_str()),
        "{context}: message of {}: {}",
        expected.id,
        resp.2
    );
    let marked: Vec<&str> = resp.1["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry["field"].as_str())
        .collect();
    assert_eq!(
        marked,
        expected
            .field
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "{context}: {SCREEN_ATTRS_ROW}: the text of {} marks its audited BMS field: {}",
        expected.id,
        resp.2
    );
    let fields: Vec<(&str, &str)> = expected
        .field
        .iter()
        .map(|field| (field.as_str(), expected.message.as_str()))
        .collect();
    assert_error(resp, expected.status, code, &expected.message, &fields);
    assert_error_body(&resp.1, context);
}

/// The keys of the JSON object `value`; none when it is no object.
fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// Asserts that `body`, an error answer, is the error body alone: a `code`
/// and a `message`, with any `fields`, and each `fields` entry a BMS field
/// and its text only. The protection, colour and `*` markers of
/// `3300-SETUP-SCREEN-ATTRS` have no API counterpart.
fn assert_error_body(body: &Value, context: &str) {
    let found = keys(body);
    assert!(
        found.contains("code")
            && found.contains("message")
            && found.iter().all(|key| ERROR_KEYS.contains(key)),
        "{context}: {SEND_MAP_ROW}: an error answer is the error body alone: {body}"
    );
    for entry in body["fields"].as_array().into_iter().flatten() {
        assert_eq!(
            keys(entry),
            BTreeSet::from(FIELD_ENTRY_KEYS),
            "{context}: {SCREEN_ATTRS_ROW}: a field entry carries no protection, colour or \
             marker: {entry}"
        );
    }
}

impl FixtureCard {
    /// The card [`FIXTURE_ROW`] pins, its number and account checked against
    /// the forms the requests send.
    fn from_audit(doc: &Document) -> Self {
        assert_eq!(
            expected_value(doc, MISSING_ROW, "value"),
            MISSING_CARD,
            "{MISSING_ROW} pins the missing card"
        );
        let card = Self {
            card: expected_value(doc, FIXTURE_ROW, "value"),
            account: expected_value(doc, FIXTURE_ROW, "card_acct_id"),
            name: expected_value(doc, FIXTURE_ROW, "card_embossed_name"),
            expiry: expected_value(doc, FIXTURE_ROW, "card_expiraion_date"),
            status: expected_value(doc, FIXTURE_ROW, "card_active_status"),
        };
        assert_eq!(
            card.card, FIXTURE_CARD,
            "{FIXTURE_ROW} pins the fixture card"
        );
        assert_eq!(card.account.len(), 11, "{FIXTURE_ROW}: an 11-digit account");
        assert_eq!(card.expiry.len(), 10, "{FIXTURE_ROW}: a YYYY-MM-DD expiry");
        card
    }

    /// The `GET /cards/{id}` path of the card, with its account.
    fn view_path(&self) -> String {
        format!("/cards/{}?account_id={}", self.card, self.account)
    }

    /// The expiry day `(9:2)`, which `PUT /cards/{id}` never receives.
    fn expiry_day(&self) -> &str {
        &self.expiry[8..10]
    }
}

/// The string `name` of a JSON object.
fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value[name]
        .as_str()
        .unwrap_or_else(|| panic!("no string `{name}` in {value}"))
}

/// Every string of `value`, at any depth.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        Value::Object(object) => object.values().flat_map(strings).collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => Vec::new(),
    }
}

/// Whether `text` has the shape `3100-SCREEN-INIT` gives the header's
/// `CURDATEO` (mm/dd/yy) or `CURTIMEO` (hh:mm:ss).
fn is_header_clock(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 8
        && [0, 1, 3, 4, 6, 7]
            .into_iter()
            .all(|at| bytes[at].is_ascii_digit())
        && matches!(bytes[2], b'/' | b':')
        && bytes[5] == bytes[2]
}

/// One answer with the media type it was sent as.
struct Sent {
    /// The status, the body as JSON and the raw body.
    resp: Resp,
    /// The `Content-Type` header, `None` when absent or not text.
    content_type: Option<String>,
}

/// The 3270 screen a turn ends with, which `PUT /cards/{id}` replaces with
/// the one JSON answer of the request.
struct Screen {
    /// The names [`SCREEN_INIT_ROW`] quotes for the header (`TRNNAMEO` and
    /// `PGMNAMEO`) and [`SEND_SCREEN_ROW`] for the `SEND MAP` (the mapset and
    /// the map).
    names: Vec<String>,
}

impl Screen {
    /// The screen as the two rows quote it.
    fn from_audit(doc: &Document) -> Self {
        let mut names = literals(expected_cell(doc, SCREEN_INIT_ROW), SCREEN_INIT_ROW);
        names.extend(literals(
            &expected_entry(doc, SEND_SCREEN_ROW, "action"),
            SEND_SCREEN_ROW,
        ));
        Self { names }
    }

    /// Asserts that `sent` is the one answer of its request, sent as JSON: a
    /// 200 is the card, without any header field, and its text, any other
    /// status the error body alone ([`assert_error_body`]). No string of it
    /// is a screen name or has the shape of the header's date or time.
    fn assert_answered(&self, sent: &Sent, context: &str) {
        let (status, body, raw) = &sent.resp;
        assert!(
            sent.content_type
                .as_deref()
                .is_some_and(|media| media.starts_with(JSON_MEDIA_TYPE)),
            "{context}: {SEND_SCREEN_ROW}: the answer is sent as {JSON_MEDIA_TYPE}, not {:?}: \
             {raw}",
            sent.content_type
        );
        if *status == StatusCode::OK {
            assert_eq!(
                keys(body),
                BTreeSet::from(UPDATED_KEYS),
                "{context}: {SEND_MAP_ROW}: a 200 is the card and its text: {raw}"
            );
            assert_eq!(
                keys(&body["card"]),
                BTreeSet::from(CARD_KEYS),
                "{context}: {SCREEN_INIT_ROW}: the card carries no header field: {raw}"
            );
        } else {
            assert_error_body(body, context);
        }
        for shown in strings(body) {
            assert!(
                !self
                    .names
                    .iter()
                    .any(|name| name.trim_end() == shown.trim_end())
                    && !is_header_clock(shown),
                "{context}: {SCREEN_INIT_ROW}, {SEND_SCREEN_ROW}: no screen header or map name is \
                 sent: {shown:?} in {raw}"
            );
        }
    }
}

/// What `ABEND-ROUTINE` would send, and the text that replaces it.
struct Abend {
    /// The `message` of the 500 that replaces the abend, as [`ABEND_ROW`]
    /// quotes it.
    unexpected: String,
    /// The `ABEND-DATA` texts the rows quote, which no answer carries: the
    /// abend message never moved, the abend codes, the culprit and the
    /// unreachable data-scenario message.
    never_sent: Vec<String>,
}

impl Abend {
    /// The texts of [`ABEND_ROW`], [`ABEND_MSG_ROW`] and [`DECIDE_OTHER_ROW`].
    fn from_audit(doc: &Document) -> Self {
        let unexpected = expected_value(doc, ABEND_ROW, "replacement");
        assert_eq!(
            unexpected,
            messages::UNEXPECTED_ERROR,
            "{ABEND_ROW}: the catalog text is the audited text"
        );
        let mut never_sent = vec![expected_value(doc, ABEND_ROW, "ABEND-CULPRIT")];
        for (id, key) in [
            (ABEND_ROW, "rule"),
            (ABEND_ROW, "action"),
            (ABEND_MSG_ROW, "disposition"),
            (DECIDE_OTHER_ROW, "disposition"),
        ] {
            never_sent.extend(literals(&expected_entry(doc, id, key), id));
        }
        Self {
            unexpected,
            never_sent,
        }
    }
}

/// The `CCARD-AID` values the `CVCRD01Y` citation of [`STORE_PFKEY_ROW`]
/// covers: each 88-level `VALUE` literal, its trailing spaces dropped.
fn aid_values(doc: &Document) -> Vec<String> {
    assert_reviewed(doc, STORE_PFKEY_ROW);
    let (_, row) = doc
        .find_row(STORE_PFKEY_ROW)
        .unwrap_or_else(|| panic!("the parity audit has no {STORE_PFKEY_ROW} row"));
    let cited = row
        .cells
        .iter()
        .map(String::as_str)
        .flat_map(citations)
        .find(|citation| citation.path.ends_with(AID_COPYBOOK))
        .unwrap_or_else(|| panic!("{STORE_PFKEY_ROW} cites no {AID_COPYBOOK}"));
    let path = Repo::new(fixtures::repo_root())
        .resolve(&cited.path)
        .unwrap_or_else(|| panic!("{} is not in the repository", cited.path));
    let copybook = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let values: Vec<String> = copybook
        .lines()
        .skip(cited.start.saturating_sub(1))
        .take(cited.end.saturating_sub(cited.start) + 1)
        .filter_map(|line| line.split_once(AID_VALUE))
        .filter_map(|(_, rest)| rest.split_once('\''))
        .map(|(value, _)| value.trim_end().to_owned())
        .collect();
    assert_eq!(
        values.len(),
        AID_COUNT,
        "{STORE_PFKEY_ROW}: the CCARD-AID values of {}: {values:?}",
        cited.path
    );
    values
}

/// The `name` and `in` of each parameter of the `OpenAPI` operation `op`.
fn parameters(op: &Value) -> Vec<(&str, &str)> {
    op["parameters"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|parameter| (text(parameter, "name"), text(parameter, "in")))
        .collect()
}

/// One [`MAPSET`] row of the function map: its ID, key, UI control and
/// endpoint, each trimmed.
struct MapRow {
    /// The `F-` row ID.
    id: String,
    /// `Key or field`.
    key: String,
    /// `UI control`.
    control: String,
    /// `Endpoint`.
    endpoint: String,
}

/// The [`MAPSET`] rows of the function map, each of which must have passed
/// its review.
fn map_controls() -> Vec<MapRow> {
    let map = Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("the function map cannot be read: {error}"));
    let controls: Vec<MapRow> = map
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| table.cell(row, "BMS map").map(str::trim) == Some(MAPSET))
        .map(|(table, row)| {
            let cell = |name: &str| {
                table
                    .cell(row, name)
                    .unwrap_or_else(|| panic!("a {MAPSET} row has no `{name}` cell"))
                    .trim()
                    .to_owned()
            };
            MapRow {
                id: cell("ID"),
                key: cell("Key or field"),
                control: cell("UI control"),
                endpoint: cell("Endpoint"),
            }
        })
        .collect();
    assert!(!controls.is_empty(), "the function map has no {MAPSET} row");
    for control in &controls {
        assert_reviewed(&map, &control.id);
    }
    controls
}

/// Asserts that `endpoint`, a function-map `Endpoint` naming a request, is
/// an operation of `spec`, the `OpenAPI` document, with each query parameter
/// it names.
fn assert_served(spec: &Value, endpoint: &str, id: &str) {
    let (method, target) = endpoint
        .split_once(' ')
        .unwrap_or_else(|| panic!("{id}: `{endpoint}` names no method and path"));
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let op = &spec["paths"][path][method.to_ascii_lowercase()];
    assert!(
        op.is_object(),
        "{STORE_PFKEY_ROW}: {id}: the control's one request `{endpoint}` is served"
    );
    let served = parameters(op);
    for name in query
        .split('&')
        .filter_map(|pair| pair.split('=').next())
        .filter(|name| !name.is_empty())
    {
        assert!(
            served.contains(&(name, "query")),
            "{id}: `{endpoint}` takes the query parameter `{name}`: {served:?}"
        );
    }
}

impl Put {
    /// The request that repeats `card`, a card as `GET /cards/{id}` or a
    /// successful `PUT` shows it, for the account number `account`, with the
    /// card's token: the `COCRDUP` fields as `3200-SETUP-SCREEN-VARS` fills
    /// them, the expiry cut into year `(1:4)` and month `(6:2)`.
    fn repeating(card: &Value, account: &str) -> Self {
        let expiry = text(card, "card_expiraion_date");
        let field = |name: &str| Some(text(card, name).to_owned());
        Self {
            card: text(card, "card_number").to_owned(),
            body: CardUpdateRequest {
                account_id: Some(account.to_owned()),
                embossed_name: field("card_embossed_name"),
                active_status: field("card_active_status"),
                expiry_month: expiry.get(5..7).map(str::to_owned),
                expiry_year: expiry.get(..4).map(str::to_owned),
                version: field("version"),
            },
        }
    }

    /// The router path `/cards/{id}`.
    fn path(&self) -> String {
        format!("/cards/{}", self.card)
    }

    /// The JSON body, holding each field that is set and omitting the
    /// others, so an unset field is absent rather than `null`.
    ///
    /// The body is read back as the DTO, so a key that is not one of its
    /// field names fails here instead of reaching the handler as an absent
    /// field.
    fn json(&self) -> Value {
        let fields = [
            ("account_id", &self.body.account_id),
            ("embossed_name", &self.body.embossed_name),
            ("active_status", &self.body.active_status),
            ("expiry_month", &self.body.expiry_month),
            ("expiry_year", &self.body.expiry_year),
            ("version", &self.body.version),
        ];
        let object: Map<String, Value> = fields
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_ref()
                    .map(|text| ((*name).to_owned(), Value::String(text.clone())))
            })
            .collect();
        let json = Value::Object(object);
        let read: CardUpdateRequest = serde_json::from_value(json.clone())
            .unwrap_or_else(|error| panic!("{json} is no `CardUpdateRequest`: {error}"));
        let read_back = [
            &read.account_id,
            &read.embossed_name,
            &read.active_status,
            &read.expiry_month,
            &read.expiry_year,
            &read.version,
        ];
        assert_eq!(
            read_back,
            fields.map(|(_, value)| value),
            "the body keys are the `CardUpdateRequest` field names: {json}"
        );
        json
    }

    /// The body of [`Put::json`] with the `extra` keys added, which no
    /// `CardUpdateRequest` field reads.
    fn json_with(&self, extra: &[(&str, &str)]) -> Value {
        let mut json = self.json();
        if let Value::Object(object) = &mut json {
            for (key, value) in extra {
                object.insert((*key).to_owned(), Value::String((*value).to_owned()));
            }
        }
        json
    }
}

/// One signed-on user's view of the fixture card on one seeded database.
struct Ctx<'a> {
    /// The served router.
    app: &'a TestApp,
    /// The bearer token of the regular user `USER0001`: any signed-on user
    /// may update any card.
    token: String,
    /// The fixture card as the parity audit pins it.
    fixture: FixtureCard,
}

impl Ctx<'_> {
    /// Sends `put`.
    async fn put(&self, put: &Put) -> Resp {
        self.app
            .put(&put.path(), Some(&self.token), &put.json())
            .await
    }

    /// Sends `PUT path` with the JSON `body` under the bearer `token` and
    /// keeps the media type of the answer, through the harness's bounded
    /// client.
    async fn exchange(&self, path: &str, token: &str, body: &Value) -> Sent {
        let url = self.app.url(path);
        let response = self
            .app
            .client()
            .put(&url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .unwrap_or_else(|error| panic!("PUT {url} failed: {error}"));
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let raw = response
            .text()
            .await
            .unwrap_or_else(|error| panic!("the answer to PUT {url} cannot be read: {error}"));
        let json = serde_json::from_str(&raw).unwrap_or(Value::Null);
        Sent {
            resp: (status, json, raw),
            content_type,
        }
    }

    /// The fixture card as `GET /cards/{id}` shows it, with its token: the
    /// answer without its `message`, the information line `COCRDSLC` shows
    /// beside a found card, which the card of a `PUT` answer does not carry.
    async fn fetch(&self) -> Value {
        let (status, mut card, raw) = self
            .app
            .get(&self.fixture.view_path(), Some(&self.token))
            .await;
        assert_eq!(status, StatusCode::OK, "GET of the fixture card: {raw}");
        let info = card
            .as_object_mut()
            .and_then(|object| object.remove("message"));
        assert_eq!(
            info.as_ref().and_then(Value::as_str),
            Some(messages::DISPLAYING_REQUESTED_DETAILS),
            "GET of the fixture card shows the found card's information line: {raw}"
        );
        card
    }

    /// The fixture card fetched, after checking that it shows the values
    /// [`FIXTURE_ROW`] pins (P-CVACT02Y-009), and the request that repeats
    /// it unchanged.
    async fn unchanged(&self) -> (Value, Put) {
        let fetched = self.fetch().await;
        let fixture = &self.fixture;
        self.assert_shown(&fetched, &fixture.name, &fixture.expiry, &fixture.status);
        let version = text(&fetched, "version");
        assert!(
            version.len() == 16
                && version
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
            "the version token is 16 lower-case hex digits: {fetched}"
        );
        let unchanged = Put::repeating(&fetched, &fixture.account);
        (fetched, unchanged)
    }

    /// Asserts that `card`, as a response shows it, is the fixture card with
    /// `name`, `expiry` and `status`.
    fn assert_shown(&self, card: &Value, name: &str, expiry: &str, status: &str) {
        let fixture = &self.fixture;
        let shown = [
            "card_number",
            "account_id",
            "card_embossed_name",
            "card_expiraion_date",
            "card_active_status",
        ]
        .map(|field| text(card, field));
        assert_eq!(
            shown,
            [
                fixture.card.as_str(),
                fixture.account.as_str(),
                name,
                expiry,
                status
            ],
            "the card shown: {card}"
        );
    }

    /// Every column of the fixture card's row in its `PostgreSQL` text form,
    /// the verification code included: what a request that writes nothing
    /// must leave unchanged.
    async fn row(&self) -> Vec<Vec<Option<String>>> {
        self.app.stack().fetch_text_rows(CARD_ROW_SQL).await
    }

    /// Asserts that `card`, the answer of a rewrite, shows `name`, `expiry`
    /// and `status`, and that the card master stores them with the fixture
    /// account.
    ///
    /// The verification code is left out, as [`STORED_CARD_SQL`] states;
    /// requests that write nothing compare it through [`Ctx::row`], and
    /// [`one_request_from_any_session_writes`] holds a rewrite to the
    /// replacement [`CVV_ROW`] records, which keeps the stored code.
    async fn assert_stored(&self, card: &Value, name: &str, expiry: &str, status: &str) {
        self.assert_shown(card, name, expiry, status);
        let stored = self.app.stack().fetch_text_rows(STORED_CARD_SQL).await;
        let expected = [self.fixture.account.as_str(), name, expiry, status]
            .map(|value| Some(value.to_owned()))
            .to_vec();
        assert_eq!(stored, vec![expected], "the stored card");
    }

    /// Sends `put`, a change every edit passes, and returns the card it
    /// leaves: 200 with `committed`, the `P-COCRDUPC-007` text, and the card
    /// with a token that a new `GET` shows as well.
    async fn commit(&self, put: &Put, committed: &str) -> Value {
        let (status, body, raw) = self.put(put).await;
        assert_eq!(status, StatusCode::OK, "the rewrite: {raw}");
        assert_eq!(
            body["message"].as_str(),
            Some(committed),
            "the text of the rewrite: {raw}"
        );
        let fetched = self.fetch().await;
        assert_eq!(
            body["card"], fetched,
            "the rewritten card and its token, as a new GET shows them"
        );
        fetched
    }
}

/// Every `COCRDUPC` validation row has an input in this file, and every row
/// the file relies on passed its review: the edit table, the outcome rows and
/// the fault rows together are exactly the program's V-IDs. The SQL naming
/// the fixture card, the text the refused commit raises, the parity rows the
/// tests prove and the escalation of the missing token are checked too. No
/// container is needed.
#[test]
fn every_cocrdupc_row_is_exercised() {
    let validation = validation_audit();
    let defined: BTreeSet<String> = validation_ids(PROGRAM).into_iter().collect();
    let exercised: BTreeSet<String> = EDIT_CASES
        .iter()
        .map(|case| case.id)
        .chain(OUTCOME_ROWS)
        .chain(FAULTS.iter().map(|fault| fault.id))
        .map(str::to_owned)
        .collect();
    assert_eq!(
        exercised, defined,
        "the inputs of this file cover exactly the {PROGRAM} validation rows"
    );
    for id in &defined {
        assert_reviewed(&validation, id);
    }
    let quoted_card = format!("'{FIXTURE_CARD}'");
    for sql in FIXTURE_SQL {
        assert!(sql.contains(&quoted_card), "{sql} names the fixture card");
    }
    assert!(
        REFUSE_COMMIT_SQL.contains(&format!("'{REFUSAL_TEXT}'")),
        "the refused commit raises {REFUSAL_TEXT:?}"
    );
    let parity = parity_audit();
    for id in PARITY_ROWS {
        assert!(
            parity.find_row(id).is_some(),
            "the parity audit has no {id}"
        );
        assert_reviewed(&parity, id);
    }
    let escalations = Repo::new(fixtures::repo_root())
        .escalations()
        .unwrap_or_else(|error| panic!("the escalation log cannot be read: {error}"));
    let (table, row) = escalations
        .find_row(VERSION_ESCALATION)
        .unwrap_or_else(|| panic!("the escalation log has no {VERSION_ESCALATION}"));
    let status = table.cell(row, "Status").unwrap_or_default().trim();
    assert!(
        status.starts_with("resolved"),
        "{VERSION_ESCALATION} decides the missing token: {status}"
    );
}

/// V-COCRDUPC-001 to V-COCRDUPC-007 and V-COCRDUPC-010 to V-COCRDUPC-017,
/// each input of [`EDIT_CASES`] answered with its row's status, code, text
/// and BMS field (P-COCRDUPC-010 to P-COCRDUPC-016, P-COCRDUPC-023 for the
/// card number no card has, P-CVACT02Y-010); then the pairs of
/// [`ORDER_PROBES`] (P-COCRDUPC-002). No rejected request writes.
#[test]
fn key_and_card_field_edits_answer_with_their_audited_texts() {
    let validation = validation_audit();
    let fixture = FixtureCard::from_audit(&parity_audit());
    let app = TestApp::start();
    block_on(async {
        let ctx = Ctx {
            app: &app,
            token: app.user_token().await,
            fixture,
        };
        let (_, unchanged) = ctx.unchanged().await;
        let before = ctx.row().await;
        for case in &EDIT_CASES {
            let expected = answer(&validation, case.id);
            assert_eq!(
                expected.message, case.message,
                "{}: the catalog text is the audited text",
                case.id
            );
            let mut put = unchanged.clone();
            (case.change)(&mut put);
            let context = format!("{} with {put:?}", case.id);
            assert_answer(&ctx.put(&put).await, &expected, &context);
        }
        first_failing_edit_answers(&ctx, &validation, &unchanged).await;
        assert_eq!(ctx.row().await, before, "a rejected update writes nothing");
    });
}

/// P-COCRDUPC-002: with two values failing their edits, the row of the lower
/// `(Order, ID)` answers, at its own field. Each pair of [`ORDER_PROBES`]
/// combines the first input [`EDIT_CASES`] gives each row.
async fn first_failing_edit_answers(ctx: &Ctx<'_>, doc: &Document, unchanged: &Put) {
    let probes: Vec<(&Case, Answer)> = ORDER_PROBES
        .iter()
        .map(|id| {
            let case = EDIT_CASES
                .iter()
                .find(|case| case.id == *id)
                .unwrap_or_else(|| panic!("no input triggers {id}"));
            (case, answer(doc, id))
        })
        .collect();
    for (index, (first, first_answer)) in probes.iter().enumerate() {
        for (second, second_answer) in &probes[index + 1..] {
            let mut put = unchanged.clone();
            (first.change)(&mut put);
            (second.change)(&mut put);
            let winner = if (first_answer.order, first.id) <= (second_answer.order, second.id) {
                first_answer
            } else {
                second_answer
            };
            let context = format!("{} with {}", first.id, second.id);
            assert_answer(&ctx.put(&put).await, winner, &context);
        }
    }
}

/// V-COCRDUPC-009, the no-change answer; the rewrites and the text they
/// answer with (P-COCRDUPC-006, P-COCRDUPC-007); and V-COCRDUPC-019, the
/// change check against the token (P-COCRDUPC-005).
#[test]
fn unchanged_changed_and_concurrently_changed_cards() {
    let validation = validation_audit();
    let parity = parity_audit();
    let fixture = FixtureCard::from_audit(&parity);
    let committed = expected_value(&parity, COMMITTED_ROW, "info");
    assert_eq!(
        committed,
        messages::CHANGES_COMMITTED_TO_DATABASE,
        "{COMMITTED_ROW}: the catalog text is the audited text"
    );
    let no_change = answer(&validation, NO_CHANGE_ROW);
    let conflict = answer(&validation, CONFLICT_ROW);
    assert_eq!(
        conflict.message,
        messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
        "{CONFLICT_ROW}: the catalog text is the audited text"
    );
    let app = TestApp::start();
    block_on(async {
        let ctx = Ctx {
            app: &app,
            token: app.user_token().await,
            fixture,
        };
        let (fetched, unchanged) = ctx.unchanged().await;
        no_change_writes_nothing(&ctx, &fetched, &unchanged, &no_change).await;
        let redated = rewrites_store_the_entered_values(&ctx, &unchanged, &committed).await;
        concurrent_changes(&ctx, &redated, &conflict, &committed).await;
    });
}

/// V-COCRDUPC-009: a body whose card fields equal the fetched ones after
/// `FUNCTION UPPER-CASE` of both is answered 200 with the no-change text and
/// the card as fetched, with its token, and nothing is written. The fetched
/// name is compared upper-cased (P-COCRDUPC-022), and the account number is
/// no part of the compared card data: the card is read by its number alone
/// (P-COCRDUPC-023, E-55), so another account answers the same. The card
/// shown is the one `9000-READ-DATA` fetched, as [`SCREEN_VARS_ROW`] names
/// it for a turn that changes nothing.
async fn no_change_writes_nothing(
    ctx: &Ctx<'_>,
    fetched: &Value,
    unchanged: &Put,
    expected: &Answer,
) {
    assert_eq!(expected.status, StatusCode::OK, "{} is a 200", expected.id);
    assert_eq!(expected.code, None, "{} sets no error flag", expected.id);
    assert_eq!(
        expected.message,
        messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
        "{}: the catalog text is the audited text",
        expected.id
    );
    let mut upper_cased = unchanged.clone();
    upper_cased.body.embossed_name = unchanged
        .body
        .embossed_name
        .as_deref()
        .map(str::to_ascii_uppercase);
    let mut other_account = unchanged.clone();
    enter(&mut other_account.body.account_id, OTHER_ACCOUNT);
    let before = ctx.row().await;
    let puts = [
        ("the fetched values", unchanged),
        ("the name upper-cased", &upper_cased),
        ("another account number", &other_account),
    ];
    for (case, put) in puts {
        let (status, body, raw) = ctx.put(put).await;
        assert_eq!(status, expected.status, "{case}: {raw}");
        assert_eq!(
            body["message"].as_str(),
            Some(expected.message.as_str()),
            "{case}: {raw}"
        );
        assert_eq!(
            body["card"], *fetched,
            "{case}: {SCREEN_VARS_ROW}: the card shown is the card as fetched: {raw}"
        );
    }
    assert_eq!(ctx.row().await, before, "no change writes nothing");
}

/// P-COCRDUPC-006, P-COCRDUPC-007: a change every edit passes is rewritten,
/// answered with the committed text and a new token.
///
/// - The name is stored as entered, in mixed case (P-COCRDUPC-013).
/// - The expiry date is the entered year and month with the fetched day,
///   which the map never receives (P-COCRDUPC-010), and the status is the
///   entered one.
///
/// Returns the re-dated card as a new `GET` shows it.
async fn rewrites_store_the_entered_values(
    ctx: &Ctx<'_>,
    unchanged: &Put,
    committed: &str,
) -> Value {
    let fixture = &ctx.fixture;
    let mut renamed = unchanged.clone();
    enter(&mut renamed.body.embossed_name, CHANGED_NAME);
    let card = ctx.commit(&renamed, committed).await;
    ctx.assert_stored(&card, CHANGED_NAME, &fixture.expiry, &fixture.status)
        .await;
    assert_ne!(
        Some(text(&card, "version")),
        unchanged.body.version.as_deref(),
        "a rewrite issues a new token"
    );
    let (month, year) = NEW_EXPIRY;
    let mut redated = Put::repeating(&card, &fixture.account);
    enter(&mut redated.body.expiry_month, month);
    enter(&mut redated.body.expiry_year, year);
    enter(&mut redated.body.active_status, NEW_STATUS);
    let card = ctx.commit(&redated, committed).await;
    let expiry = format!("{year}-{month}-{}", fixture.expiry_day());
    ctx.assert_stored(&card, CHANGED_NAME, &expiry, NEW_STATUS)
        .await;
    card
}

/// P-COCRDUPC-005: the token stands for the fields `9300-CHECK-CHANGE-IN-REC`
/// compares, the name upper-cased as `9000-READ-DATA` stores it
/// (P-COCRDUPC-022).
///
/// - Another session changing only the case of the name since the fetch
///   leaves the token valid, and the update is rewritten.
/// - V-COCRDUPC-019: another session changing the name since the fetch makes
///   the token stale: 409 with the conflict text at its field, and nothing is
///   written.
async fn concurrent_changes(ctx: &Ctx<'_>, latest: &Value, conflict: &Answer, committed: &str) {
    let fixture = &ctx.fixture;
    ctx.app.stack().exec_sql(UPPER_CASE_NAME_SQL).await;
    let mut renamed = Put::repeating(latest, &fixture.account);
    enter(&mut renamed.body.embossed_name, &fixture.name);
    let restored = ctx.commit(&renamed, committed).await;
    ctx.assert_stored(
        &restored,
        &fixture.name,
        text(latest, "card_expiraion_date"),
        text(latest, "card_active_status"),
    )
    .await;

    ctx.app.stack().exec_sql(RENAME_ELSEWHERE_SQL).await;
    let before = ctx.row().await;
    let mut stale = Put::repeating(&restored, &fixture.account);
    enter(&mut stale.body.embossed_name, CHANGED_NAME);
    let resp = ctx.put(&stale).await;
    assert_answer(&resp, conflict, "a name changed by another session");
    assert_eq!(ctx.row().await, before, "a stale token writes nothing");
}

/// V-COCRDUPC-008, V-COCRDUPC-018 and V-COCRDUPC-020: a failing fetch, lock
/// or rewrite of the card master answers 500 with its audited text at
/// `ACCTSID` and leaves the card as it was (P-COCRDUPC-004, P-COCRDUPC-006,
/// P-COCRDUPC-023). Once mended, the card master takes the same update.
#[test]
fn card_file_failures_answer_with_their_audited_texts() {
    let validation = validation_audit();
    let parity = parity_audit();
    let fixture = FixtureCard::from_audit(&parity);
    let committed = expected_value(&parity, COMMITTED_ROW, "info");
    let expected: Vec<Answer> = FAULTS
        .iter()
        .map(|fault| fault_answer(&validation, fault))
        .collect();
    let app = TestApp::start();
    block_on(async {
        let ctx = Ctx {
            app: &app,
            token: app.user_token().await,
            fixture,
        };
        let (_, unchanged) = ctx.unchanged().await;
        let mut renamed = unchanged.clone();
        enter(&mut renamed.body.embossed_name, CHANGED_NAME);
        let before = ctx.row().await;
        for (fault, answer) in FAULTS.iter().zip(&expected) {
            let stack = ctx.app.stack();
            stack.exec_sql(fault.inject).await;
            let resp = ctx.put(&renamed).await;
            stack.exec_sql(fault.restore).await;
            assert_answer(&resp, answer, "an injected card master failure");
            assert_eq!(ctx.row().await, before, "{}: nothing is written", fault.id);
        }
        let card = ctx.commit(&renamed, &committed).await;
        let fixture = &ctx.fixture;
        ctx.assert_stored(&card, CHANGED_NAME, &fixture.expiry, &fixture.status)
            .await;
    });
}

/// [`ANY_KEY_ROW`], [`VALIDATED_ROW`] and [`STORE_PFKEY_ROW`] in the
/// `OpenAPI` document and the function map: no AID key reaches the API
/// (E-56).
///
/// - `PUT /cards/{id}` takes the card number as its one parameter and the
///   map fields with the token as its body, and no key.
/// - Each `COCRDUP` control of the function map is one request the document
///   serves; the other AID keys, which the COBOL turns into ENTER, have no
///   control and send nothing.
/// - An ENTER row and the PF5 row send the same `PUT /cards/{id}`: ENTER's
///   validation and the PF5 write are the one request.
///
/// No container is needed.
#[test]
fn no_aid_key_reaches_the_api_and_each_map_control_is_one_request() {
    let parity = parity_audit();
    for id in [ANY_KEY_ROW, VALIDATED_ROW, STORE_PFKEY_ROW] {
        assert_reviewed(&parity, id);
    }
    let spec: Value = serde_json::from_str(&spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is no JSON: {error}"));
    let update = &spec["paths"]["/cards/{id}"]["put"];
    assert_eq!(
        parameters(update),
        [("id", "path")],
        "{STORE_PFKEY_ROW}: {UPDATE_ENDPOINT} takes the card number, and no key, as its \
         parameter: {update}"
    );
    let schema = update["requestBody"]["content"][JSON_MEDIA_TYPE]["schema"]["$ref"]
        .as_str()
        .and_then(|reference| reference.rsplit('/').next())
        .unwrap_or_else(|| panic!("{UPDATE_ENDPOINT} has no JSON body schema: {update}"));
    let body = &spec["components"]["schemas"][schema]["properties"];
    assert_eq!(
        keys(body),
        BTreeSet::from(UPDATE_BODY_KEYS),
        "{STORE_PFKEY_ROW}, {ANY_KEY_ROW}: the body is the map fields and the token, and no \
         key: {body}"
    );
    let controls = map_controls();
    for control in &controls {
        if !control.endpoint.starts_with(NONE_CELL) {
            assert_served(&spec, &control.endpoint, &control.id);
        }
    }
    let others: Vec<&MapRow> = controls
        .iter()
        .filter(|control| control.key.starts_with(OTHER_KEYS))
        .collect();
    assert!(
        !others.is_empty()
            && others
                .iter()
                .all(|control| control.control == NONE_CELL && control.endpoint == NONE_CELL),
        "{ANY_KEY_ROW}: no control sends any other key, so the update answers it as ENTER"
    );
    let sends_update = |key: &str| {
        controls
            .iter()
            .any(|control| control.key.starts_with(key) && control.endpoint == UPDATE_ENDPOINT)
    };
    assert!(
        sends_update(ENTER_KEY) && sends_update(SAVE_KEY),
        "{VALIDATED_ROW}: ENTER's validation and the PF5 write are the one {UPDATE_ENDPOINT}"
    );
}

/// The keys and the session reach no part of the update, through the router:
///
/// - [`ANY_KEY_ROW`], [`STORE_PFKEY_ROW`]: a request naming any `CCARD-AID`
///   value answers exactly as the request naming none, with the ENTER edits'
///   answer, and writes nothing;
/// - [`COMMAREA_ROW`], [`VALIDATED_ROW`]: the token, not a session, carries
///   the fetched values, so another user's session sends the first user's
///   token and its one request edits and writes, answered with the committed
///   text and never the PF5 prompt;
/// - [`SCREEN_VARS_ROW`], [`CVV_ROW`]: the card shown is the entered values
///   with the fetched day, and the stored verification code is kept whatever
///   the body names;
/// - [`SEND_MAP_ROW`], [`SCREEN_INIT_ROW`], [`SEND_SCREEN_ROW`]: each answer
///   is one JSON body without screen header or map name.
#[test]
fn keys_and_sessions_reach_no_part_of_the_update() {
    let validation = validation_audit();
    let parity = parity_audit();
    let fixture = FixtureCard::from_audit(&parity);
    let aids = aid_values(&parity);
    let screen = Screen::from_audit(&parity);
    let invalid_name = answer(&validation, NAME_PROBE);
    let texts = [COMMITTED_ROW, VALIDATED_ROW].map(|id| expected_value(&parity, id, "info"));
    assert_eq!(
        texts[0],
        messages::CHANGES_COMMITTED_TO_DATABASE,
        "{COMMITTED_ROW}: the catalog text is the audited text"
    );
    for id in [COMMAREA_ROW, SCREEN_VARS_ROW, CVV_ROW] {
        assert_reviewed(&parity, id);
    }
    let app = TestApp::start();
    block_on(async {
        let ctx = Ctx {
            app: &app,
            token: app.user_token().await,
            fixture,
        };
        let (_, unchanged) = ctx.unchanged().await;
        any_key_answers_as_enter(&ctx, &unchanged, &aids, &invalid_name, &screen).await;
        one_request_from_any_session_writes(&ctx, &unchanged, &texts, &screen).await;
    });
}

/// [`ANY_KEY_ROW`], [`STORE_PFKEY_ROW`]: the update has no key input, so a
/// body key and a query parameter naming any `CCARD-AID` value, the PF3 exit
/// and the PF12 re-read included, change nothing. Each request repeats the
/// `expected` row's input and answers exactly as the request naming no key,
/// with that ENTER edit's answer, and nothing is written.
async fn any_key_answers_as_enter(
    ctx: &Ctx<'_>,
    unchanged: &Put,
    aids: &[String],
    expected: &Answer,
    screen: &Screen,
) {
    let case = EDIT_CASES
        .iter()
        .find(|case| case.id == expected.id)
        .unwrap_or_else(|| panic!("no input triggers {}", expected.id));
    let mut put = unchanged.clone();
    (case.change)(&mut put);
    let before = ctx.row().await;
    let plain = ctx.exchange(&put.path(), &ctx.token, &put.json()).await;
    assert_answer(&plain.resp, expected, "the request naming no key");
    screen.assert_answered(&plain, "the request naming no key");
    for aid in aids {
        let path = format!("{}?{AID_KEY}={aid}", put.path());
        let body = put.json_with(&[(AID_KEY, aid.as_str())]);
        let keyed = ctx.exchange(&path, &ctx.token, &body).await;
        assert_eq!(
            (keyed.resp.0, &keyed.resp.1),
            (plain.resp.0, &plain.resp.1),
            "{ANY_KEY_ROW}, {STORE_PFKEY_ROW}: the key {aid} reaches no part of the update, which \
             answers as ENTER: {}",
            keyed.resp.2
        );
    }
    assert_eq!(
        ctx.row().await,
        before,
        "{STORE_PFKEY_ROW}: no key makes a rejected request write"
    );
}

/// [`COMMAREA_ROW`], [`VALIDATED_ROW`], [`SCREEN_VARS_ROW`], [`CVV_ROW`]: the
/// state the COMMAREA kept between turns travels in the request, and
/// `texts` are the committed text and the PF5 prompt.
///
/// - A second GET issues the same token and neither GET writes: the token
///   stands for the fetched values, not for a session.
/// - The admin user's session sends the regular user's fetched token with a
///   change every edit passes and a key naming a verification code: the one
///   request edits and writes, answered with the committed text, never the
///   PF5 prompt, and with the token a new GET issues.
/// - The card shown and stored is the entered name, status, month and year
///   with the fetched day, and the stored verification code is kept.
async fn one_request_from_any_session_writes(
    ctx: &Ctx<'_>,
    unchanged: &Put,
    texts: &[String; 2],
    screen: &Screen,
) {
    let [committed, validated] = texts;
    let before = ctx.row().await;
    let again = ctx.fetch().await;
    assert_eq!(
        Some(text(&again, "version")),
        unchanged.body.version.as_deref(),
        "{COMMAREA_ROW}: the token stands for the fetched values, so a second GET issues it again"
    );
    assert_eq!(
        ctx.row().await,
        before,
        "{COMMAREA_ROW}: a GET writes nothing"
    );
    let stored_cvv = before[0][CVV_COLUMN].clone();
    assert_ne!(
        stored_cvv.as_deref(),
        Some(PROBE_CVV),
        "the probe code differs from the stored one"
    );
    let (month, year) = NEW_EXPIRY;
    let mut changed = unchanged.clone();
    enter(&mut changed.body.embossed_name, CHANGED_NAME);
    enter(&mut changed.body.expiry_month, month);
    enter(&mut changed.body.expiry_year, year);
    enter(&mut changed.body.active_status, NEW_STATUS);
    let admin = ctx.app.admin_token().await;
    let body = changed.json_with(&[(CVV_KEY, PROBE_CVV)]);
    let sent = ctx.exchange(&changed.path(), &admin, &body).await;
    screen.assert_answered(&sent, "the one request of another session");
    let (status, body, raw) = &sent.resp;
    assert_eq!(
        *status,
        StatusCode::OK,
        "{COMMAREA_ROW}: another session's request with the fetched token edits and writes: {raw}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(committed.as_str()),
        "{VALIDATED_ROW}: edits that pass are written by the one request: {raw}"
    );
    assert_ne!(
        body["message"].as_str(),
        Some(validated.as_str()),
        "{VALIDATED_ROW}: no validated-but-unsaved answer exists (E-56): {raw}"
    );
    let expiry = format!("{year}-{month}-{}", ctx.fixture.expiry_day());
    let card = &body["card"];
    assert_eq!(
        [
            "card_embossed_name",
            "card_expiraion_date",
            "card_active_status"
        ]
        .map(|name| text(card, name)),
        [CHANGED_NAME, expiry.as_str(), NEW_STATUS],
        "{SCREEN_VARS_ROW}: the card shown is the entered name, month, year and status with the \
         fetched day: {raw}"
    );
    ctx.assert_stored(card, CHANGED_NAME, &expiry, NEW_STATUS)
        .await;
    assert_eq!(
        ctx.row().await[0][CVV_COLUMN],
        stored_cvv,
        "{CVV_ROW}: the update writes no verification code, whatever the body names"
    );
    assert_eq!(
        ctx.fetch().await,
        *card,
        "{COMMAREA_ROW}: the answer carries the token a new GET issues"
    );
}

/// The order of the inputs and the failure no rule chose a text for, through
/// the router:
///
/// - [`PROCESS_INPUTS_ROW`]: with the card master hidden, a body that does
///   not parse is refused before the key edits run, a key edit answers
///   before any file is read, and a request the key edits pass reaches the
///   hidden file;
/// - [`ABEND_ROW`], [`ABEND_MSG_ROW`], [`DECIDE_OTHER_ROW`]: a commit refused
///   after the rewrite is the 500 'Unexpected error' without fields, and the
///   answer carries none of the `ABEND-DATA` texts; the next request is
///   served as before.
#[test]
fn inputs_parse_before_the_edits_and_no_abend_data_is_sent() {
    let validation = validation_audit();
    let parity = parity_audit();
    let fixture = FixtureCard::from_audit(&parity);
    let screen = Screen::from_audit(&parity);
    let abend = Abend::from_audit(&parity);
    let committed = expected_value(&parity, COMMITTED_ROW, "info");
    assert_reviewed(&parity, PROCESS_INPUTS_ROW);
    let key_edit = answer(&validation, KEY_PROBE);
    let hidden = FAULTS
        .iter()
        .find(|fault| fault.id == FETCH_FAULT)
        .unwrap_or_else(|| panic!("no fault answers {FETCH_FAULT}"));
    let unreadable = fault_answer(&validation, hidden);
    let app = TestApp::start();
    block_on(async {
        let ctx = Ctx {
            app: &app,
            token: app.user_token().await,
            fixture,
        };
        let (_, unchanged) = ctx.unchanged().await;
        let mut renamed = unchanged.clone();
        enter(&mut renamed.body.embossed_name, CHANGED_NAME);
        let expected = [&key_edit, &unreadable];
        inputs_parse_then_edit_before_any_file(&ctx, &renamed, hidden, expected, &screen).await;
        unexpected_failure_sends_no_abend_data(&ctx, &renamed, &abend, &screen).await;
        let card = ctx.commit(&renamed, &committed).await;
        let fixture = &ctx.fixture;
        ctx.assert_stored(&card, CHANGED_NAME, &fixture.expiry, &fixture.status)
            .await;
    });
}

/// [`PROCESS_INPUTS_ROW`]: `1100-RECEIVE-MAP`, then `1200-EDIT-MAP-INPUTS`,
/// with no file access. With the card master hidden as `hidden` hides it:
///
/// - a body holding the key edit's input and a name that is a JSON number is
///   refused 400 `INVALID`, naming the name and no BMS field: the body
///   parses before the key edits run;
/// - the same body with its name as text answers with the key edit's text,
///   the first of `expected`: the edits answer before any file is read;
/// - `renamed`, which every key edit passes, answers with the fetch's file
///   error, the second of `expected`: the file was hidden.
async fn inputs_parse_then_edit_before_any_file(
    ctx: &Ctx<'_>,
    renamed: &Put,
    hidden: &Fault,
    [key_edit, unreadable]: [&Answer; 2],
    screen: &Screen,
) {
    let case = EDIT_CASES
        .iter()
        .find(|case| case.id == key_edit.id)
        .unwrap_or_else(|| panic!("no input triggers {}", key_edit.id));
    let mut bad_key = renamed.clone();
    (case.change)(&mut bad_key);
    let mut unparsed = bad_key.json();
    unparsed[UNPARSED_FIELD] = Value::from(UNPARSED_NAME);
    let stack = ctx.app.stack();
    stack.exec_sql(hidden.inject).await;
    let refused = ctx.exchange(&bad_key.path(), &ctx.token, &unparsed).await;
    let edited = ctx
        .exchange(&bad_key.path(), &ctx.token, &bad_key.json())
        .await;
    let read = ctx
        .exchange(&renamed.path(), &ctx.token, &renamed.json())
        .await;
    stack.exec_sql(hidden.restore).await;
    for (sent, context) in [
        (&refused, "a body that does not parse"),
        (&edited, "a key edit"),
        (&read, "a hidden card master"),
    ] {
        screen.assert_answered(sent, context);
    }
    let (status, body, raw) = &refused.resp;
    assert_eq!(
        (*status, body["code"].as_str(), body.get("fields")),
        (
            StatusCode::BAD_REQUEST,
            Some(ErrorKind::Invalid.code()),
            None
        ),
        "{PROCESS_INPUTS_ROW}: the body parses first, before the key edits and any file access: \
         {raw}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.contains(UNPARSED_FIELD)),
        "{PROCESS_INPUTS_ROW}: the refusal names the body field that does not parse: {raw}"
    );
    let context = format!("{PROCESS_INPUTS_ROW}: the key edits answer before any file access");
    assert_answer(&edited.resp, key_edit, &context);
    let context = format!("{PROCESS_INPUTS_ROW}: a request the key edits pass reads the card file");
    assert_answer(&read.resp, unreadable, &context);
}

/// [`ABEND_ROW`], [`ABEND_MSG_ROW`], [`DECIDE_OTHER_ROW`]: a commit refused
/// after the rewrite is a failure no rule chose a text for. Its answer
/// replaces the abend: 500, kind `Unexpected`, `abend.unexpected` and no
/// field, carrying none of the `ABEND-DATA` texts and no detail of the
/// failure.
async fn unexpected_failure_sends_no_abend_data(
    ctx: &Ctx<'_>,
    renamed: &Put,
    abend: &Abend,
    screen: &Screen,
) {
    let stack = ctx.app.stack();
    stack.exec_sql(REFUSE_COMMIT_SQL).await;
    let sent = ctx
        .exchange(&renamed.path(), &ctx.token, &renamed.json())
        .await;
    stack.exec_sql(ACCEPT_COMMIT_SQL).await;
    screen.assert_answered(&sent, "a refused commit");
    let (status, body, raw) = &sent.resp;
    assert_eq!(
        (
            *status,
            body["code"].as_str(),
            body["message"].as_str(),
            body.get("fields")
        ),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some(ErrorKind::Unexpected.code()),
            Some(abend.unexpected.as_str()),
            None
        ),
        "{ABEND_ROW}: an unexpected failure is the 500 unexpected-error answer: {raw}"
    );
    for sent_text in &abend.never_sent {
        assert!(
            !raw.contains(sent_text.trim_end()),
            "{ABEND_ROW}, {ABEND_MSG_ROW}, {DECIDE_OTHER_ROW}: no ABEND-DATA text is sent: \
             {sent_text:?} in {raw}"
        );
    }
    assert!(
        !raw.contains(REFUSAL_TEXT),
        "{ABEND_ROW}: the answer carries no detail of the failure: {raw}"
    );
}
