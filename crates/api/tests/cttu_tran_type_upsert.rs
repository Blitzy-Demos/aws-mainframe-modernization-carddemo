//! CTTU over the Db2 transaction type master: `POST /admin/transaction-types`,
//! `PUT /admin/transaction-types/{id}` without `origin` and
//! `DELETE /admin/transaction-types/{id}?origin=form`, the `COTRTUPC` add,
//! update and F4 delete of one type
//! [app/app-transaction-type-db2/cbl/COTRTUPC.cbl], table `transaction_types`
//! [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4].
//!
//! Every answer is checked against its row of `docs/parity/validation-audit.md`:
//! the status, the kind (the error body's `code`), the BMS field and the Order
//! are read from the row, and each expected text is composed by
//! `carddemo_domain::messages` and first compared with the row's message cell,
//! its `{…}` slots filled. Record-state and Db2 outcomes are those of section B
//! of `docs/parity/parity-oracle-audit.md`, and the seeded values the cases
//! start from are read from its fixture rows.
//!
//! - The key and description edits of `1200-EDIT-MAP-INPUTS`: one input table
//!   keyed by V-ID for the three operations, the values longer than their BMS
//!   field, and the lower Order answering when two edits fail. Each error
//!   names the field `3300-SETUP-SCREEN-ATTRS` puts the cursor on in the
//!   state the failure leaves (P-COTRTUPC-025).
//! - The add, which `9600-WRITE-PROCESSING` makes an UPDATE that finds no row
//!   and then `9700-INSERT-RECORD`, and the 409 without a message for a code
//!   already on file (E-51).
//! - The update: a missing type, no change, and a committed change that reaches
//!   the master `transaction_types` and not the processing copy `trantype_file`,
//!   which the reports read until `batch seed --only tranextr` copies the
//!   master over it; `crates/batch/tests/tranextr_refresh.rs` runs that batch
//!   half.
//! - Read and write failures, injected with triggers, a failing view over the
//!   master and a master taken away, answered with the Db2 texts and the code
//!   of the condition the failure maps to (E-30), and a failure with no Db2
//!   code answered as the `Unexpected` kind.
//! - The pseudo-conversation the requests replace (E-56, E-63, E-64, E-69):
//!   the attention keys are the methods the function map names, and no body
//!   member is read as a key; the add and the delete are one request each;
//!   the edits run before the read and the read before the write; no request
//!   leaves a state the next one meets; and an answer is the JSON body of its
//!   DTO, carrying the values the screen would show in its state.
//! - F3: the `Back` target of the function map is the list route of the admin
//!   menu in `web/src/nav/menu.ts`.
//! - The legend (P-COTRTUPC-026): each key is the control of the function map
//!   rows citing its `COTRTUP.bms` field, and no answer carries the legend or
//!   an attribute field.
//! - The F4 delete, `origin=form` (E-23): `Delete successful.` with the type
//!   as read, the `COTRTUPC` Db2 texts of a refused or failed delete, and the
//!   `COTRTLIC` texts that `DELETE` keeps without `origin`.
//!
//! `every_cotrtupc_row_is_exercised_here` fails when the audit holds a
//! `COTRTUPC` row that no case of this file sends a request for.
//!
//! Every test but `every_cotrtupc_row_is_exercised_here` and
//! `the_exit_key_returns_to_the_list_route_of_the_admin_menu` needs a running
//! Docker daemon.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use carddemo_api::dto::admin_transaction_types::{
    TranTypeCreateRequest, TranTypeMessageResponse, TranTypeResponse, TranTypeUpdateRequest,
    TranTypeUpsertField,
};
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, Repo, cell_code_spans, code_span, is_no_message, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages::{
    CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY, CHANGES_COMMITTED_TO_DATABASE,
    COULD_NOT_LOCK_RECORD_FOR_UPDATE, DELETE_SUCCESSFUL, FIELD_TRAN_TYPE_CODE,
    FIELD_TRANSACTION_DESC, METHOD_NOT_ALLOWED, MUST_BE_NUMERIC, MUST_BE_SUPPLIED,
    MUST_NOT_BE_ZERO, NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
    NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE, TranTypeSqlFailure, UNEXPECTED_ERROR,
    field_edit_message, tran_type_sql_message, with_sqlcode,
};
use carddemo_domain::ports::PortError;
use carddemo_testkit::{block_on, fixtures};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::header::CONTENT_TYPE;
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::Serialize;
use serde_json::{Map, Value};

/// The add route, and the prefix of the update route.
const TYPES_PATH: &str = "/admin/transaction-types";

/// The Db2 master as `COTRTUPC` reads and writes it, in key order.
const MASTER_ROWS: &str = "SELECT tr_type, tr_description FROM transaction_types ORDER BY tr_type";

/// The processing copy `TRANTYPE`, in key order: the reports read it, and no
/// online write reaches it.
const PROCESSING_COPY_ROWS: &str =
    "SELECT tran_type, tran_type_desc FROM trantype_file ORDER BY tran_type";

/// The slot of the field label in the edit texts:
/// `FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)`
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L864-868].
const EDIT_VARIABLE_NAME: &str = "WS-EDIT-VARIABLE-NAME";

/// The slot of the edited `SQLCODE`, `WS-DISP-SQLCODE PIC ----9`
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L68].
const DISP_SQLCODE: &str = "WS-DISP-SQLCODE";

/// The slot of `SQLERRM OF SQLCA` in the Db2 texts.
const SQLERRM: &str = "SQLERRM";

/// What the `SQLERRM` slot carries: nothing, as no database text may reach a
/// response (E-30).
const SQLERRM_TEXT: &str = "";

/// The `message` of an answer no COBOL text covers: an input longer than its
/// BMS field, which the 3270 field could not hold.
const NO_MESSAGE: &str = "";

/// A valid description for the cases whose key is under test: letters and a
/// space, as `1230-EDIT-ALPHANUM-REQD` allows.
const GIFT_CARD: Option<&str> = Some("GIFT CARD");

/// A description holding `-`, which `1230-EDIT-ALPHANUM-REQD` refuses: the
/// audit's example of V-COTRTUPC-008 and V-COTRTUPC-018.
const PURCHASE_RETAIL: Option<&str> = Some("PURCHASE-RETAIL");

/// One character more than `TRTYDSCI PIC X(50)`
/// [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L66].
const DESCRIPTION_51: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWXY";

/// The length [`DESCRIPTION_51`] must have.
const DESCRIPTION_51_LEN: usize = 51;

/// `9100-GET-TRANSACTION-TYPE` answering +100 on the update.
const NOT_FOUND: &str = "V-COTRTUPC-004";

/// `1205-COMPARE-OLD-NEW` finding no change: 200, nothing written.
const NO_CHANGE: &str = "V-COTRTUPC-006";

/// The blank description of the update, Order 6, which a missing type
/// precedes.
const BLANK_DESCRIPTION_ON_UPDATE: &str = "V-COTRTUPC-007";

/// The add of a code already on file: 409 without a message (E-51).
// No Db2 -803 text is expected: the read finds the code before any INSERT is
// tried, and the COBOL shows that row for update and sets no message, so the
// answer carries none.
const DUPLICATE: &str = "V-COTRTUPC-015";

/// The alphanumeric description edit of the add, Order 6, which a code on
/// file precedes.
const INVALID_DESCRIPTION_ON_ADD: &str = "V-COTRTUPC-018";

/// `9100-GET-TRANSACTION-TYPE` answering +100 on the F4 delete: F4 needs the
/// details shown, so nothing is deleted.
const NOT_FOUND_ON_DELETE: &str = "V-COTRTUPC-025";

/// The F4 delete refused with SQLCODE -532: categories reference the type.
const RESTRICTED_DELETE: &str = "V-COTRTUPC-027";

/// The record-state rows the add, update and delete tests exercise outside
/// the tables below.
const RECORD_STATE_ROWS: [&str; 5] = [
    NOT_FOUND,
    NO_CHANGE,
    DUPLICATE,
    NOT_FOUND_ON_DELETE,
    RESTRICTED_DELETE,
];

/// The program segment of the V-IDs this file answers for.
const PROGRAM: &str = "COTRTUPC";

/// The query that selects the `COTRTUPC` F4 delete on
/// `DELETE /admin/transaction-types/{id}` (E-23); without it the `COTRTLIC`
/// row delete answers.
const FORM_ORIGIN_QUERY: &str = "?origin=form";

/// `9800-DELETE-PROCESSING` with `SQLCODE` 0: `Delete successful.`, and the
/// `COTRTLIC` text of the same delete.
const P_DELETED: &str = "P-COTRTUPC-004";

/// `9800-DELETE-PROCESSING` refused with -532 or failed: the two composed
/// `COTRTUPC` texts.
const P_DELETE_FAILURES: &str = "P-COTRTUPC-005";

/// `3250-SETUP-INFOMSG`: the info line by state.
const P_INFO_LINE: &str = "P-COTRTUPC-024";

/// `3300-SETUP-SCREEN-ATTRS`: the field the cursor is put on, by state.
const P_SCREEN_ATTRS: &str = "P-COTRTUPC-025";

/// The cursor clause of [`P_SCREEN_ATTRS`] whose states put the cursor on the
/// key `TRTYPCD`.
const CURSOR_ON_KEY: &str = "cursor on the key for ";

/// The cursor clause of [`P_SCREEN_ATTRS`] whose states put the cursor on the
/// description `TRTYDSC`.
const CURSOR_ON_DESCRIPTION: &str = "on the description for ";

/// The cursor clause of [`P_SCREEN_ATTRS`] for every other state.
const CURSOR_OTHERWISE: &str = "else on the key";

/// The state a key edit leaves, as [`P_SCREEN_ATTRS`] names it.
const KEY_FLAG_STATE: &str = "a key flag not OK or blank";

/// The state a description edit leaves, as [`P_SCREEN_ATTRS`] names it.
const DESCRIPTION_FLAG_STATE: &str = "a description flag not OK or blank";

/// The state a read leaves that finds no type or fails: key not found (X),
/// for SQLCODE +100 and a failed SELECT alike (P-COTRTUPC-019).
const KEY_NOT_FOUND_STATE: &str = "key not found";

/// The state a lock on the UPDATE leaves: `TTUP-CHANGES-OKAYED-LOCK-ERROR`
/// ('L'), one of the changes-made states.
const LOCK_ERROR_STATE: &str = "changes made (E, N, L, F)";

/// The state an UPDATE or INSERT failure leaves: its text overwrites
/// `TABLE-UPDATE-FAILED`, so the done state ('C') is set (E-51).
const UPDATE_DONE_STATE: &str = "update done";

/// The states a refused or failed delete leaves, '8' after -532 (E-69) and
/// '6' otherwise, which no clause of [`P_SCREEN_ATTRS`] names.
const DELETE_STATE: &str = "else";

/// The state prefix of the delete-done text in [`P_INFO_LINE`].
const INFO_DELETE_DONE: &str = "delete done ->";

/// The state prefix of the update-done text in [`P_INFO_LINE`].
const INFO_UPDATE_DONE: &str = "update done ->";

/// `9600-WRITE-PROCESSING` with `SQLCODE` 0: `Changes committed to database`,
/// the description stored as `FUNCTION TRIM` of the input.
const P_COMMITTED: &str = "P-COTRTUPC-007";

/// The UPDATE outcomes: +100 goes on to the INSERT, -911 is the lock text,
/// any other negative code the update text.
const P_UPDATE_OUTCOMES: &str = "P-COTRTUPC-008";

/// `9700-INSERT-RECORD`: committed, or the insert text.
const P_INSERT: &str = "P-COTRTUPC-009";

/// `1150-STORE-MAP-IN-NEW`: `*` or spaces are no input, anything else is
/// trimmed.
const P_STORE_MAP: &str = "P-COTRTUPC-014";

/// The key edits, and the valid key stored by `FUNCTION NUMVAL` as two digits.
const P_KEY_EDITS: &str = "P-COTRTUPC-015";

/// The compare with the fetched type, then the description edit.
const P_COMPARE: &str = "P-COTRTUPC-016";

/// `1230-EDIT-ALPHANUM-REQD` of the description.
const P_ALPHANUM_EDIT: &str = "P-COTRTUPC-017";

/// `1245-EDIT-NUM-REQD` of the key.
const P_NUM_EDIT: &str = "P-COTRTUPC-018";

/// `9000-READ-TRANTYPE`: the fetched description the compare uses.
const P_FETCHED: &str = "P-COTRTUPC-020";

/// `9100-GET-TRANSACTION-TYPE`: +100 is the not-found text, a negative code
/// the access text.
const P_READ: &str = "P-COTRTUPC-021";

/// The add of a code on file stays 409 and changes no row (E-70, as
/// proposed).
const P_ADD_ON_FILE: &str = "P-COTRTUPC-031";

/// `0001-CHECK-PFKEYS`: the keys each state accepts, any other answered
/// with the invalid-key text, which E-56 does not reproduce.
const P_KEYS: &str = "P-COTRTUPC-001";

/// F3: `SYNCPOINT`, then `XCTL` to the calling program or the admin menu.
const P_EXIT: &str = "P-COTRTUPC-002";

/// F4 with the details shown: the delete confirmation, no write.
const P_ASK_DELETE: &str = "P-COTRTUPC-003";

/// F5 on a key not found: a new record is created.
const P_ADD_KEY: &str = "P-COTRTUPC-006";

/// F12 on a pending delete or on validated changes: cancelled, no write.
const P_CANCEL: &str = "P-COTRTUPC-010";

/// After a finished or failed update or delete, any key is a fresh entry:
/// the typed input and the message are dropped.
const P_RESET: &str = "P-COTRTUPC-011";

/// `COMMON-RETURN`: the COMMAREA carries the state to the next key; the
/// replacement keeps none, each request carrying the code (E-56).
const P_COMMAREA: &str = "P-COTRTUPC-012";

/// `1000-PROCESS-INPUTS`: the map is stored and edited before
/// `2000-DECIDE-ACTION` reads.
const P_PROCESS_INPUTS: &str = "P-COTRTUPC-013";

/// `2000-DECIDE-ACTION`: a valid key is read, found or not found (+100 and
/// a failed SELECT alike); the abend of a delete state is reached by no
/// request.
const P_DECIDE: &str = "P-COTRTUPC-019";

/// `3000-SEND-MAP`: the screen header and layout, free in the SPA.
const P_SEND_MAP: &str = "P-COTRTUPC-022";

/// `3200-SETUP-SCREEN-VARS`: the values shown, by state.
const P_SHOWN_VALUES: &str = "P-COTRTUPC-023";

/// `ABEND-ROUTINE`: no request reaches the data-scenario abend, and an
/// unexpected failure is the `Unexpected` kind.
const P_ABEND: &str = "P-COTRTUPC-027";

/// F4 and F6: carried as `DELETE ?origin=form` and `POST`; `origin=form`
/// selects the `COTRTUPC` delete messages, and `DELETE` without `origin`
/// keeps the `COTRTLIC` ones (E-23).
const P_DELETE_AND_ADD: &str = "P-COTRTUPC-028";

/// The branches no input reaches (E-68), among them the never-set
/// `DATA-WAS-CHANGED-BEFORE-UPDATE`: the replacement has no path for them.
const P_UNREACHED: &str = "P-COTRTUPC-029";

/// A delete refused with SQLCODE -532, then ENTER or F5: the server keeps no
/// delete state, so no later request meets the abend (E-69).
const P_RESTRICTED_DELETE: &str = "P-COTRTUPC-030";

/// The seeded Db2 master row `01` `PURCHASE` from `DB2LTTYP`.
const P_SEEDED_TYPE: &str = "P-TRNTYPE-004";

/// The code no seeded type has, `99`.
const P_MISSING_TYPE: &str = "P-TRNTYPE-005";

/// The seeded processing copy row `01` `Purchase` from `trantype.txt`.
const P_PROCESSING_TYPE: &str = "P-CVTRA03Y-005";

/// Every parity row this file relies on.
const PARITY_ROWS: [&str; 33] = [
    P_DELETED,
    P_DELETE_FAILURES,
    P_INFO_LINE,
    P_SCREEN_ATTRS,
    P_COMMITTED,
    P_UPDATE_OUTCOMES,
    P_INSERT,
    P_STORE_MAP,
    P_KEY_EDITS,
    P_COMPARE,
    P_ALPHANUM_EDIT,
    P_NUM_EDIT,
    P_FETCHED,
    P_READ,
    P_ADD_ON_FILE,
    P_KEYS,
    P_EXIT,
    P_ASK_DELETE,
    P_ADD_KEY,
    P_CANCEL,
    P_RESET,
    P_COMMAREA,
    P_PROCESS_INPUTS,
    P_DECIDE,
    P_SEND_MAP,
    P_SHOWN_VALUES,
    P_ABEND,
    P_DELETE_AND_ADD,
    P_UNREACHED,
    P_RESTRICTED_DELETE,
    P_SEEDED_TYPE,
    P_MISSING_TYPE,
    P_PROCESSING_TYPE,
];

/// The `BMS map` cell of the function map rows of the `COTRTUP` screen.
const COTRTUP_MAP: &str = "COTRTUP";

/// The function map row of F3, the `Back` link.
const F_BACK: &str = "F-COTRTUP-005";

/// The `Endpoint` cell of a function map row that sends no request.
const NAVIGATION_ENDPOINT: &str = "— (navigation)";

/// The single navigation definition of the SPA, holding the admin menu.
const MENU_FILE: &str = "web/src/nav/menu.ts";

/// The menu entry of the type list, the program F3 returns to from the list.
const LIST_PROGRAM_ENTRY: &str = "program: \"COTRTLIC\",";

/// The section of the admin menu `COADM01C`, the other program F3 returns to.
const ADMIN_SECTION_ENTRY: &str = "section: \"admin\",";

/// The update route as the function map names it.
const TYPE_PATH_TEMPLATE: &str = "/admin/transaction-types/{id}";

/// The methods sent to each route: the four the function map names and
/// `PATCH`, which no `COTRTUP` control sends.
const METHODS: [Method; 5] = [
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::DELETE,
    Method::PATCH,
];

/// A body member a 3270 request would carry as its attention key (`EIBAID`).
const AID_MEMBER: &str = "aid";

/// The keys of the `COTRTUP` legend and one it does not offer, each sent as
/// [`AID_MEMBER`].
const AID_KEYS: [&str; 7] = ["ENTER", "PF3", "PF4", "PF5", "PF6", "PF12", "PA1"];

/// The code the add and delete case creates: no seeded type has it, so no
/// category references it.
const NEW_CODE: &str = "08";

/// A second code no seeded type has, which the `COTRTLIC` delete removes.
const LISTED_NEW_CODE: &str = "09";

/// The look-up of [`NEW_CODE`] on the key entry route: the list filtered by
/// the code.
const NEW_CODE_LOOKUP: &str = "/admin/transaction-types?type_code=08";

/// A seeded type other than `01`, whose description another unit of work
/// changes.
const OTHER_CODE: &str = "02";

/// The look-up of [`OTHER_CODE`].
const OTHER_CODE_LOOKUP: &str = "/admin/transaction-types?type_code=02";

/// Another unit of work changing the description of [`OTHER_CODE`] after the
/// client showed it.
const CHANGE_OTHER_ELSEWHERE: &str =
    "UPDATE transaction_types SET tr_description = 'CHANGED ELSEWHERE' WHERE tr_type = '02'";

/// The members of a success answer.
const ANSWER_MEMBERS: [&str; 2] = ["message", "transaction_type"];

/// The members of the type an answer carries.
const TYPE_MEMBERS: [&str; 2] = ["description", "type_code"];

/// The members of an error answer that names a field.
const ERROR_MEMBERS: [&str; 3] = ["code", "fields", "message"];

/// The media type of every answer.
const JSON_MEDIA_TYPE: &str = "application/json";

/// One request of a case: the `COTRTUP` fields it carries, as entered.
#[derive(Clone, Copy, Debug)]
enum Request<'a> {
    /// `POST /admin/transaction-types`; `None` sends the member as `null`,
    /// which the body reads as the absent, empty field.
    Add {
        code: Option<&'a str>,
        description: Option<&'a str>,
    },
    /// `PUT /admin/transaction-types/{code}` without `origin`.
    Update {
        code: &'a str,
        description: Option<&'a str>,
    },
    /// `DELETE /admin/transaction-types/{code}?origin=form`, the confirmed
    /// F4; it has no body.
    Delete { code: &'a str },
}

/// The add of `code` and `description`.
const fn add<'a>(code: Option<&'a str>, description: Option<&'a str>) -> Request<'a> {
    Request::Add { code, description }
}

/// The update of the type `code` with `description`.
const fn update<'a>(code: &'a str, description: Option<&'a str>) -> Request<'a> {
    Request::Update { code, description }
}

/// The F4 delete of the type `code`.
const fn delete(code: &str) -> Request<'_> {
    Request::Delete { code }
}

/// One input of an edit row, and the label and edit text
/// `field_edit_message` composes the row's message from.
#[derive(Debug)]
struct EditCase {
    id: &'static str,
    request: Request<'static>,
    label: &'static str,
    suffix: &'static str,
}

/// An input of the key edit row `id` (`1245-EDIT-NUM-REQD`, P-COTRTUPC-018).
const fn key_edit(id: &'static str, request: Request<'static>, suffix: &'static str) -> EditCase {
    EditCase {
        id,
        request,
        label: FIELD_TRAN_TYPE_CODE,
        suffix,
    }
}

/// An input of the description edit row `id` (`1230-EDIT-ALPHANUM-REQD`,
/// P-COTRTUPC-017).
const fn description_edit(
    id: &'static str,
    request: Request<'static>,
    suffix: &'static str,
) -> EditCase {
    EditCase {
        id,
        request,
        label: FIELD_TRANSACTION_DESC,
        suffix,
    }
}

/// The inputs of every edit row of the three operations. The update's and the
/// delete's key is the `{id}` segment; the update's description cases address
/// type `01`, which is on file (P-TRNTYPE-004), so the read passes and the
/// compare finds a change. The add's description cases use `99`, which is not
/// (P-TRNTYPE-005). `*` and spaces are no input (P-COTRTUPC-014), and `0` is a
/// zero that `NUMVAL` reads.
const EDIT_CASES: [EditCase; 34] = [
    key_edit("V-COTRTUPC-001", update("  ", GIFT_CARD), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-001", update(" ", GIFT_CARD), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-001", update("*", GIFT_CARD), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-002", update("AB", GIFT_CARD), MUST_BE_NUMERIC),
    key_edit("V-COTRTUPC-002", update("0A", GIFT_CARD), MUST_BE_NUMERIC),
    key_edit("V-COTRTUPC-003", update("00", GIFT_CARD), MUST_NOT_BE_ZERO),
    key_edit("V-COTRTUPC-003", update("0", GIFT_CARD), MUST_NOT_BE_ZERO),
    description_edit("V-COTRTUPC-007", update("01", Some("")), MUST_BE_SUPPLIED),
    description_edit("V-COTRTUPC-007", update("01", None), MUST_BE_SUPPLIED),
    description_edit(
        "V-COTRTUPC-007",
        update("01", Some("   ")),
        MUST_BE_SUPPLIED,
    ),
    description_edit("V-COTRTUPC-007", update("01", Some("*")), MUST_BE_SUPPLIED),
    description_edit(
        "V-COTRTUPC-008",
        update("01", PURCHASE_RETAIL),
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ),
    description_edit(
        "V-COTRTUPC-008",
        update("01", Some("RETAIL/PURCHASE")),
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ),
    key_edit("V-COTRTUPC-012", add(None, GIFT_CARD), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-012", add(Some(""), GIFT_CARD), MUST_BE_SUPPLIED),
    key_edit(
        "V-COTRTUPC-012",
        add(Some("  "), GIFT_CARD),
        MUST_BE_SUPPLIED,
    ),
    key_edit(
        "V-COTRTUPC-012",
        add(Some("*"), GIFT_CARD),
        MUST_BE_SUPPLIED,
    ),
    key_edit(
        "V-COTRTUPC-013",
        add(Some("AB"), GIFT_CARD),
        MUST_BE_NUMERIC,
    ),
    key_edit(
        "V-COTRTUPC-013",
        add(Some("0A"), GIFT_CARD),
        MUST_BE_NUMERIC,
    ),
    key_edit(
        "V-COTRTUPC-014",
        add(Some("00"), GIFT_CARD),
        MUST_NOT_BE_ZERO,
    ),
    key_edit(
        "V-COTRTUPC-014",
        add(Some("0"), GIFT_CARD),
        MUST_NOT_BE_ZERO,
    ),
    description_edit(
        "V-COTRTUPC-017",
        add(Some("99"), Some("")),
        MUST_BE_SUPPLIED,
    ),
    description_edit("V-COTRTUPC-017", add(Some("99"), None), MUST_BE_SUPPLIED),
    description_edit(
        "V-COTRTUPC-017",
        add(Some("99"), Some("*")),
        MUST_BE_SUPPLIED,
    ),
    description_edit(
        "V-COTRTUPC-018",
        add(Some("99"), PURCHASE_RETAIL),
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ),
    description_edit(
        "V-COTRTUPC-018",
        add(Some("99"), Some("GIFT-CARD")),
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ),
    description_edit(
        "V-COTRTUPC-018",
        add(Some("99"), Some("Café")),
        CAN_HAVE_NUMBERS_OR_ALPHABETS_ONLY,
    ),
    key_edit("V-COTRTUPC-022", delete("  "), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-022", delete(" "), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-022", delete("*"), MUST_BE_SUPPLIED),
    key_edit("V-COTRTUPC-023", delete("AB"), MUST_BE_NUMERIC),
    key_edit("V-COTRTUPC-023", delete("0A"), MUST_BE_NUMERIC),
    key_edit("V-COTRTUPC-024", delete("00"), MUST_NOT_BE_ZERO),
    key_edit("V-COTRTUPC-024", delete("0"), MUST_NOT_BE_ZERO),
];

/// A request two edits reject: `answers`, the input of the row of the lower
/// Order, which answers alone, as the COBOL moves a text only while
/// `WS-RETURN-MSG` is blank and ends the edits at a failing key; and `loses`,
/// the row of the other edit.
#[derive(Debug)]
struct PrecedenceCase {
    answers: EditCase,
    loses: &'static str,
}

/// Each pair of a key edit and a description edit, per operation.
const PRECEDENCE_CASES: [PrecedenceCase; 4] = [
    PrecedenceCase {
        answers: key_edit(
            "V-COTRTUPC-002",
            update("AB", PURCHASE_RETAIL),
            MUST_BE_NUMERIC,
        ),
        loses: "V-COTRTUPC-008",
    },
    PrecedenceCase {
        answers: key_edit("V-COTRTUPC-003", update("00", Some("")), MUST_NOT_BE_ZERO),
        loses: "V-COTRTUPC-007",
    },
    PrecedenceCase {
        answers: key_edit(
            "V-COTRTUPC-012",
            add(Some("  "), PURCHASE_RETAIL),
            MUST_BE_SUPPLIED,
        ),
        loses: "V-COTRTUPC-018",
    },
    PrecedenceCase {
        answers: key_edit("V-COTRTUPC-013", add(Some("AB"), Some("")), MUST_BE_NUMERIC),
        loses: "V-COTRTUPC-017",
    },
];

/// An input longer than its BMS field (`TRTYPCDI PIC X(2)`, `TRTYDSCI PIC
/// X(50)` [app/app-transaction-type-db2/cpy-bms/COTRTUP.cpy:L60-66]): no COBOL
/// path reaches the edits with it, so it is 400 `INVALID` with no message on
/// each field it names, before any edit or read.
#[derive(Debug)]
struct OverLengthCase {
    request: Request<'static>,
    fields: &'static [TranTypeUpsertField],
}

/// Over-length keys and descriptions, alone, together, and beside a blank
/// description that the edits would refuse.
const OVER_LENGTH_CASES: [OverLengthCase; 7] = [
    OverLengthCase {
        request: add(Some("123"), GIFT_CARD),
        fields: &[TranTypeUpsertField::TrTypCd],
    },
    OverLengthCase {
        request: add(Some("123"), Some("")),
        fields: &[TranTypeUpsertField::TrTypCd],
    },
    OverLengthCase {
        request: add(Some("99"), Some(DESCRIPTION_51)),
        fields: &[TranTypeUpsertField::TrTyDsc],
    },
    OverLengthCase {
        request: add(Some("123"), Some(DESCRIPTION_51)),
        fields: &[TranTypeUpsertField::TrTypCd, TranTypeUpsertField::TrTyDsc],
    },
    OverLengthCase {
        request: update("123", GIFT_CARD),
        fields: &[TranTypeUpsertField::TrTypCd],
    },
    OverLengthCase {
        request: update("123", Some("")),
        fields: &[TranTypeUpsertField::TrTypCd],
    },
    OverLengthCase {
        request: update("01", Some(DESCRIPTION_51)),
        fields: &[TranTypeUpsertField::TrTyDsc],
    },
];

/// The functions the faults call: a trigger function that fails its
/// statement with the SQLSTATE given as its argument, a row trigger function
/// that skips the row, and a predicate that fails the query evaluating it with
/// `55P03`, a lock not available.
const FAULT_FUNCTIONS: &str = "
CREATE FUNCTION cttu_fail_statement() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'cttu injected statement failure' USING ERRCODE = TG_ARGV[0];
END $$;
CREATE FUNCTION cttu_skip_row() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RETURN NULL;
END $$;
CREATE FUNCTION cttu_fail_read() RETURNS boolean LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'cttu injected read failure' USING ERRCODE = '55P03';
END $$";

/// Every UPDATE of the master fails with `55P03`, a lock not available, as
/// the statement starts, whether it would match a row or not.
const LOCK_ON_UPDATE: &str = "CREATE TRIGGER cttu_fault BEFORE UPDATE ON transaction_types \
     FOR EACH STATEMENT EXECUTE FUNCTION cttu_fail_statement('55P03')";

/// Every UPDATE of the master fails with `23505`, a unique violation, as the
/// statement starts.
const FAIL_ON_UPDATE: &str = "CREATE TRIGGER cttu_fault BEFORE UPDATE ON transaction_types \
     FOR EACH STATEMENT EXECUTE FUNCTION cttu_fail_statement('23505')";

/// Every UPDATE of the master skips its row, so it changes none and reports
/// none: the row was there for the read, and the UPDATE finds no row.
const SKIP_ROW_UPDATE: &str = "CREATE TRIGGER cttu_fault BEFORE UPDATE ON transaction_types \
     FOR EACH ROW EXECUTE FUNCTION cttu_skip_row()";

/// Every INSERT into the master fails with `P0001`, an error no Db2 code
/// stands for.
const FAIL_ON_INSERT: &str = "CREATE TRIGGER cttu_fault BEFORE INSERT ON transaction_types \
     FOR EACH STATEMENT EXECUTE FUNCTION cttu_fail_statement('P0001')";

/// Every DELETE from the master fails with `55P03`, a lock not available, as
/// the statement starts, before any category reference is checked.
const LOCK_ON_DELETE: &str = "CREATE TRIGGER cttu_fault BEFORE DELETE ON transaction_types \
     FOR EACH STATEMENT EXECUTE FUNCTION cttu_fail_statement('55P03')";

/// Removes the trigger of a write fault.
const DROP_FAULT_TRIGGER: &str = "DROP TRIGGER cttu_fault ON transaction_types";

/// Puts a view in the master's place whose every row read fails with `55P03`.
/// A read finds no row to fail on for a code not on file, so the read cases
/// address `01` (P-TRNTYPE-004).
const FAIL_ON_READ: &str = "
ALTER TABLE transaction_types RENAME TO cttu_transaction_types_held;
CREATE VIEW transaction_types AS
    SELECT tr_type, tr_description FROM cttu_transaction_types_held WHERE cttu_fail_read()";

/// Puts the master back in place of the [`FAIL_ON_READ`] view.
const RESTORE_READ: &str = "
DROP VIEW transaction_types;
ALTER TABLE cttu_transaction_types_held RENAME TO transaction_types";

/// Takes the master away: every statement naming it fails with `42P01`, an
/// undefined table, whatever code it addresses, and the port reports that
/// with no Db2 code.
const HIDE_MASTER: &str = "ALTER TABLE transaction_types RENAME TO cttu_transaction_types_held";

/// Puts the master back after [`HIDE_MASTER`].
const RESTORE_MASTER: &str = "ALTER TABLE cttu_transaction_types_held RENAME TO transaction_types";

/// A failure injected on the master for one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    LockOnUpdate,
    FailOnUpdate,
    SkipRowUpdate,
    FailOnInsert,
    LockOnDelete,
    FailOnRead,
    MasterMissing,
}

impl Fault {
    /// The statements that install the fault.
    const fn inject(self) -> &'static str {
        match self {
            Self::LockOnUpdate => LOCK_ON_UPDATE,
            Self::FailOnUpdate => FAIL_ON_UPDATE,
            Self::SkipRowUpdate => SKIP_ROW_UPDATE,
            Self::FailOnInsert => FAIL_ON_INSERT,
            Self::LockOnDelete => LOCK_ON_DELETE,
            Self::FailOnRead => FAIL_ON_READ,
            Self::MasterMissing => HIDE_MASTER,
        }
    }

    /// The statements that remove it.
    const fn restore(self) -> &'static str {
        match self {
            Self::FailOnRead => RESTORE_READ,
            Self::MasterMissing => RESTORE_MASTER,
            Self::LockOnUpdate
            | Self::FailOnUpdate
            | Self::SkipRowUpdate
            | Self::FailOnInsert
            | Self::LockOnDelete => DROP_FAULT_TRIGGER,
        }
    }
}

/// The condition a failure maps to, whose Db2 code a text shows (E-30).
#[derive(Clone, Copy, Debug)]
enum Condition {
    /// No row: `SQLCODE` +100.
    NotFound,
    /// A unique violation: -803.
    Duplicate,
    /// A lock not available: -911.
    Contention,
    /// A row other rows still reference: -532.
    Restricted,
}

impl Condition {
    /// The Db2 code of the condition, as the port reports it.
    fn sqlcode(self) -> i32 {
        let error = match self {
            Self::NotFound => PortError::NotFound,
            Self::Duplicate => PortError::Duplicate,
            Self::Contention => PortError::Contention,
            Self::Restricted => PortError::Restricted,
        };
        error
            .sqlcode()
            .unwrap_or_else(|| panic!("{error:?} stands for no Db2 code"))
    }
}

/// The text of a failure row.
#[derive(Clone, Copy, Debug)]
enum Db2Text {
    /// `Could not lock record for update`, the -911 of the UPDATE.
    Lock,
    /// The composed text of the failure, its `SQLCODE` slot the Db2 code of
    /// the condition and its `SQLERRM` slot [`SQLERRM_TEXT`].
    Sql(TranTypeSqlFailure, Condition),
}

/// One read or write failure row: the request, the fault it meets, the text
/// it is answered with, and the state it leaves, as [`P_SCREEN_ATTRS`] names
/// it for the cursor.
#[derive(Debug)]
struct FailureCase {
    id: &'static str,
    request: Request<'static>,
    fault: Fault,
    text: Db2Text,
    cursor: &'static str,
}

/// The read and write failure rows of the three operations. The update and
/// the delete address seeded types, which are on file; the add's write cases
/// use `08`, which is not, so its UPDATE finds no row before the INSERT
/// (P-COTRTUPC-008, P-COTRTUPC-009), and the INSERT text keeps that +100
/// whatever the INSERT answered (E-51).
const FAILURE_CASES: [FailureCase; 10] = [
    FailureCase {
        id: "V-COTRTUPC-005",
        request: update("01", Some("FAILED READ")),
        fault: Fault::FailOnRead,
        text: Db2Text::Sql(TranTypeSqlFailure::Access, Condition::Contention),
        cursor: KEY_NOT_FOUND_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-009",
        request: update("01", Some("LOCKED UPDATE")),
        fault: Fault::LockOnUpdate,
        text: Db2Text::Lock,
        cursor: LOCK_ERROR_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-010",
        request: update("01", Some("FAILED UPDATE")),
        fault: Fault::FailOnUpdate,
        text: Db2Text::Sql(TranTypeSqlFailure::Update, Condition::Duplicate),
        cursor: UPDATE_DONE_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-011",
        request: update("01", Some("SKIPPED UPDATE")),
        fault: Fault::SkipRowUpdate,
        text: Db2Text::Sql(TranTypeSqlFailure::Insert, Condition::NotFound),
        cursor: UPDATE_DONE_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-016",
        request: add(Some("01"), Some("FAILED READ")),
        fault: Fault::FailOnRead,
        text: Db2Text::Sql(TranTypeSqlFailure::Access, Condition::Contention),
        cursor: KEY_NOT_FOUND_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-019",
        request: add(Some("08"), Some("LOCKED UPDATE")),
        fault: Fault::LockOnUpdate,
        text: Db2Text::Lock,
        cursor: LOCK_ERROR_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-020",
        request: add(Some("08"), Some("FAILED UPDATE")),
        fault: Fault::FailOnUpdate,
        text: Db2Text::Sql(TranTypeSqlFailure::Update, Condition::Duplicate),
        cursor: UPDATE_DONE_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-021",
        request: add(Some("08"), Some("FAILED INSERT")),
        fault: Fault::FailOnInsert,
        text: Db2Text::Sql(TranTypeSqlFailure::Insert, Condition::NotFound),
        cursor: UPDATE_DONE_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-026",
        request: delete("01"),
        fault: Fault::FailOnRead,
        text: Db2Text::Sql(TranTypeSqlFailure::Access, Condition::Contention),
        cursor: KEY_NOT_FOUND_STATE,
    },
    FailureCase {
        id: "V-COTRTUPC-028",
        request: delete("02"),
        fault: Fault::LockOnDelete,
        text: Db2Text::Sql(TranTypeSqlFailure::Delete, Condition::Contention),
        cursor: DELETE_STATE,
    },
];

/// What one `V-COTRTUPC` row of the validation audit fixes for an answer.
#[derive(Debug)]
struct AuditRow {
    id: String,
    status: StatusCode,
    /// The `code` of the row's kind; `None` for `kind=none`, a 200 that only
    /// sets a message.
    code: Option<&'static str>,
    field: String,
    order: u32,
    message_cell: String,
}

/// The row `id` of the validation audit `doc`.
fn audit_row(doc: &Document, id: &str) -> AuditRow {
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
    let order = cell("Order")
        .parse()
        .unwrap_or_else(|error| panic!("{id} has no numeric Order: {error}"));
    AuditRow {
        id: id.to_owned(),
        status,
        code: kind_code(id, &kind),
        field: cell("BMS field").to_owned(),
        order,
        message_cell: cell("Message text").to_owned(),
    }
}

/// The error body `code` of the kind a row names.
fn kind_code(id: &str, kind: &str) -> Option<&'static str> {
    let kind = match kind {
        "none" => return None,
        "Invalid" => ErrorKind::Invalid,
        "Empty" => ErrorKind::Empty,
        "NotFound" => ErrorKind::NotFound,
        "Duplicate" => ErrorKind::Duplicate,
        "Conflict" => ErrorKind::Conflict,
        "Unexpected" => ErrorKind::Unexpected,
        other => panic!("{id} names the unknown kind `{other}`"),
    };
    Some(kind.code())
}

/// The text the message cell of `row` gives once each `{slot}` of `slots` is
/// filled: the cell's code span without its quotes, `\"` and `\\` unescaped.
/// A `no-message (E-nn)` cell gives the empty text the answer carries.
fn audited_text(row: &AuditRow, slots: &[(&str, &str)]) -> String {
    let id = &row.id;
    if is_no_message(&row.message_cell) {
        return String::new();
    }
    let span = code_span(&row.message_cell)
        .unwrap_or_else(|| panic!("{id} quotes no message: {}", row.message_cell));
    let quoted = span
        .text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} message is not a quoted string: {}", span.text));
    let mut text = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(ch) = chars.next() {
        text.extend(if ch == '\\' { chars.next() } else { Some(ch) });
    }
    for (slot, value) in slots {
        text = text.replace(&format!("{{{slot}}}"), value);
    }
    assert!(!text.contains('{'), "{id} has a slot left unfilled: {text}");
    text
}

/// The last review verdict of the row `id` in the review logs of `doc`.
fn review_verdict<'a>(doc: &'a Document, id: &str) -> Option<&'a str> {
    doc.tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(move |row| table.cell(row, "Row ID").map(str::trim) == Some(id))
                .filter_map(move |row| table.cell(row, "Verdict"))
        })
        .next_back()
        .map(str::trim)
}

/// The quoted value of `key` in the `Expected result` cell of the parity
/// fixture row `id`, without its quotes.
fn fixture_value(doc: &Document, id: &str, key: &str) -> String {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let result = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    let pair = key_values(result)
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| panic!("{id} gives no `{key}`: {result}"));
    let span = code_span(&pair.value).unwrap_or_else(|| panic!("{id} `{key}` is not quoted"));
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} `{key}` is not a quoted string: {}", span.text))
        .to_owned()
}

/// The `Expected result` cell of the parity row `id`.
fn expected_result<'a>(doc: &'a Document, id: &str) -> &'a str {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"))
}

/// The first code span of `text`, a part of a cell of the row `id`, without
/// its quotes.
fn quoted(id: &str, text: &str) -> String {
    let span = code_span(text).unwrap_or_else(|| panic!("{id} quotes nothing in `{text}`"));
    span.text
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} quotes no string in `{text}`: {}", span.text))
        .to_owned()
}

/// The parts of the `Expected result` cell of the parity row `id` between
/// its semicolons, trimmed.
fn result_clauses<'a>(doc: &'a Document, id: &str) -> Vec<&'a str> {
    expected_result(doc, id).split(';').map(str::trim).collect()
}

/// The info line text [`P_INFO_LINE`] gives in the state its clause `prefix`
/// names, such as [`INFO_DELETE_DONE`].
fn info_text(doc: &Document, prefix: &str) -> String {
    let clauses = result_clauses(doc, P_INFO_LINE);
    let clause = clauses
        .iter()
        .find(|clause| clause.starts_with(prefix))
        .unwrap_or_else(|| panic!("{P_INFO_LINE} has no `{prefix}` clause: {clauses:?}"));
    quoted(P_INFO_LINE, clause)
}

/// The BMS field [`P_SCREEN_ATTRS`] puts the cursor on in `state`: the key
/// when its key clause names the state, the description when its description
/// clause does, and the key for [`DELETE_STATE`], which neither names, by its
/// `else` clause.
fn cursor_field(doc: &Document, state: &str) -> &'static str {
    let clauses = result_clauses(doc, P_SCREEN_ATTRS);
    let clause = |prefix: &str| {
        clauses
            .iter()
            .find_map(|clause| clause.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("{P_SCREEN_ATTRS} has no `{prefix}` clause: {clauses:?}"))
    };
    let (on_key, on_description) = (clause(CURSOR_ON_KEY), clause(CURSOR_ON_DESCRIPTION));
    let key = TranTypeUpsertField::TrTypCd.as_str();
    if state == DELETE_STATE {
        assert_eq!(
            clause(CURSOR_OTHERWISE),
            "",
            "{P_SCREEN_ATTRS}: the last cursor clause is `{CURSOR_OTHERWISE}`"
        );
        assert!(
            !on_key.contains("delete") && !on_description.contains("delete"),
            "{P_SCREEN_ATTRS}: no cursor clause names a delete state: {clauses:?}"
        );
        return key;
    }
    match (on_key.contains(state), on_description.contains(state)) {
        (true, false) => key,
        (false, true) => TranTypeUpsertField::TrTyDsc.as_str(),
        _ => panic!("{P_SCREEN_ATTRS} must name `{state}` in one cursor clause: {clauses:?}"),
    }
}

/// Asserts that the BMS field of `row` is the one [`P_SCREEN_ATTRS`] puts the
/// cursor on in `state`, the state the row's failure leaves.
fn assert_cursor_field(parity: &Document, row: &AuditRow, state: &str) {
    assert_eq!(
        row.field,
        cursor_field(parity, state),
        "{P_SCREEN_ATTRS}: {} leaves `{state}`",
        row.id
    );
}

/// The text [`P_DELETE_FAILURES`] composes for `failure` with `sqlcode` in
/// the `WS-DISP-SQLCODE` slot and [`SQLERRM_TEXT`] in each `SQLERRM` slot:
/// its `error` value holds the -532 text, then the other failure's text.
fn delete_failure_text(doc: &Document, failure: TranTypeSqlFailure, sqlcode: i32) -> String {
    let id = P_DELETE_FAILURES;
    let error = key_values(expected_result(doc, id))
        .into_iter()
        .find(|pair| pair.key == "error")
        .unwrap_or_else(|| panic!("{id} gives no `error`"))
        .value;
    let templates: Vec<&str> = error.split(" / ").collect();
    let template = match (failure, templates.as_slice()) {
        (TranTypeSqlFailure::DeleteChildRecords, [child, _]) => child,
        (TranTypeSqlFailure::Delete, [_, other]) => other,
        _ => panic!("{id} gives no {failure:?} text: {error}"),
    };
    let sqlcode_slot = format!("{{{DISP_SQLCODE}}}");
    let sqlerrm_slot = format!("{{{SQLERRM}}}");
    template
        .split('+')
        .map(str::trim)
        .map(|part| {
            if part == sqlcode_slot {
                with_sqlcode("", sqlcode)
            } else if part == sqlerrm_slot {
                SQLERRM_TEXT.to_owned()
            } else {
                quoted(id, part)
            }
        })
        .collect()
}

/// The edit text of `case`, checked against its audit row: `field_edit_message`
/// of the case's label and edit text equals the row's template with the label
/// in its slot.
fn edit_text(row: &AuditRow, case: &EditCase) -> String {
    let message = field_edit_message(case.label, case.suffix);
    assert_eq!(
        audited_text(row, &[(EDIT_VARIABLE_NAME, case.label)]),
        message,
        "{}: the message catalog gives the audited text",
        row.id
    );
    message
}

/// The text of a failure row, checked against its audit row with the edited
/// `SQLCODE` and the empty `SQLERRM` in the slots.
fn failure_text(row: &AuditRow, text: Db2Text) -> String {
    let (message, sqlcode) = match text {
        Db2Text::Lock => (COULD_NOT_LOCK_RECORD_FOR_UPDATE.to_owned(), String::new()),
        Db2Text::Sql(failure, condition) => (
            tran_type_sql_message(failure, condition.sqlcode(), SQLERRM_TEXT),
            with_sqlcode("", condition.sqlcode()),
        ),
    };
    assert_eq!(
        audited_text(row, &[(DISP_SQLCODE, &sqlcode), (SQLERRM, SQLERRM_TEXT)]),
        message,
        "{}: the message catalog gives the audited text",
        row.id
    );
    message
}

/// Asserts that `resp` is the error answer of `row` with `message`, on the
/// row's BMS field.
fn assert_row_error(resp: &Resp, row: &AuditRow, message: &str) {
    let code = row
        .code
        .unwrap_or_else(|| panic!("{} is not an error row", row.id));
    assert_error(
        resp,
        row.status,
        code,
        message,
        &[(row.field.as_str(), message)],
    );
}

/// The JSON body of a request DTO.
fn json(body: &impl Serialize) -> Value {
    serde_json::to_value(body)
        .unwrap_or_else(|error| panic!("the request body serializes: {error}"))
}

/// The update and delete route of `code`: every byte but an ASCII letter or
/// digit percent-encoded, so blanks and `*` reach the `{id}` segment as
/// entered.
fn type_path(code: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut path = format!("{TYPES_PATH}/");
    for byte in code.bytes() {
        if byte.is_ascii_alphanumeric() {
            path.push(char::from(byte));
        } else {
            path.push('%');
            path.push(char::from(HEX[usize::from(byte >> 4)]));
            path.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    path
}

/// The HTTP request of `request`, its body the request DTO.
fn request_builder(app: &TestApp, request: Request<'_>) -> RequestBuilder {
    match request {
        Request::Add { code, description } => {
            let body = TranTypeCreateRequest {
                type_code: code.map(str::to_owned),
                description: description.map(str::to_owned),
            };
            app.client().post(app.url(TYPES_PATH)).json(&json(&body))
        }
        Request::Update { code, description } => {
            let body = TranTypeUpdateRequest {
                description: description.map(str::to_owned),
            };
            app.client()
                .put(app.url(&type_path(code)))
                .json(&json(&body))
        }
        Request::Delete { code } => {
            let path = format!("{}{FORM_ORIGIN_QUERY}", type_path(code));
            app.client().delete(app.url(&path))
        }
    }
}

/// Sends `request` as the admin whose bearer token is `token`.
async fn send(app: &TestApp, token: &str, request: Request<'_>) -> Resp {
    common::send(request_builder(app, request), Some(token)).await
}

/// Every row `sql` reads, each column as text.
async fn text_rows(app: &TestApp, sql: &str) -> Vec<Vec<String>> {
    app.stack()
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .map(|columns| {
            columns
                .into_iter()
                .map(|column| column.unwrap_or_else(|| panic!("{sql} read a null")))
                .collect()
        })
        .collect()
}

/// A master or processing copy row: the code and the description.
fn type_row(code: &str, description: &str) -> Vec<String> {
    vec![code.to_owned(), description.to_owned()]
}

/// `rows` with the description of `code` replaced by `description`.
fn with_description(rows: &[Vec<String>], code: &str, description: &str) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            if row.first().map(String::as_str) == Some(code) {
                type_row(code, description)
            } else {
                row.clone()
            }
        })
        .collect()
}

/// Asserts that `resp` is a `status` answer carrying the type `code` with
/// `description` and the text `message`, and no `info`, which only the
/// `COTRTLIC` inline update sends.
fn assert_answer(resp: &Resp, status: StatusCode, code: &str, description: &str, message: &str) {
    let (actual, body, raw) = resp;
    assert_eq!(*actual, status, "status of the answer: {raw}");
    assert!(
        body.get("info").is_none(),
        "a COTRTUPC answer has no info: {raw}"
    );
    let answer: TranTypeMessageResponse = serde_json::from_value(body.clone())
        .unwrap_or_else(|error| panic!("the answer is a TranTypeMessageResponse ({error}): {raw}"));
    let expected = TranTypeMessageResponse {
        transaction_type: Some(TranTypeResponse {
            type_code: code.to_owned(),
            description: description.to_owned(),
        }),
        message: message.to_owned(),
        info: None,
    };
    assert_eq!(answer, expected, "the answer: {raw}");
}

/// The IDs of every `COTRTUPC` row a request of this file is sent for.
fn exercised_ids() -> BTreeSet<String> {
    let edits = EDIT_CASES.iter().map(|case| case.id);
    let precedence = PRECEDENCE_CASES.iter().map(|case| case.answers.id);
    let failures = FAILURE_CASES.iter().map(|case| case.id);
    edits
        .chain(precedence)
        .chain(failures)
        .chain(RECORD_STATE_ROWS)
        .map(str::to_owned)
        .collect()
}

/// Every `COTRTUPC` row of the validation audit is answered by a request of
/// this file. Every row passed its review and names a field of the `COTRTUP`
/// map, and every parity row this file relies on exists and passed its
/// review. Needs no Docker.
#[test]
fn every_cotrtupc_row_is_exercised_here() {
    let doc = validation_audit();
    let exercised = exercised_ids();
    let audited: BTreeSet<String> = validation_ids(PROGRAM).into_iter().collect();
    assert_eq!(
        exercised, audited,
        "the COTRTUPC rows of the validation audit"
    );

    let fields = [TranTypeUpsertField::TrTypCd, TranTypeUpsertField::TrTyDsc]
        .map(TranTypeUpsertField::as_str);
    for id in &exercised {
        assert_eq!(
            review_verdict(&doc, id),
            Some("pass"),
            "{id} must pass review"
        );
        let row = audit_row(&doc, id);
        assert!(
            fields.contains(&row.field.as_str()),
            "{id} names `{}`, no field of TranTypeUpsertField",
            row.field
        );
    }

    let parity = parity_audit();
    for id in PARITY_ROWS {
        assert!(
            parity.find_row(id).is_some(),
            "the parity audit has no {id} row"
        );
        assert_eq!(
            review_verdict(&parity, id),
            Some("pass"),
            "{id} must pass review"
        );
    }
    assert_eq!(DESCRIPTION_51.chars().count(), DESCRIPTION_51_LEN);
}

/// The key and description edits of the three operations answer their audit
/// rows, each on the field the cursor is put on; when a key edit and a
/// description edit both fail, the row of the lower Order answers alone; an
/// input longer than its field is refused before every edit. Nothing is
/// written.
#[test]
fn key_and_description_edits_answer_their_audit_rows() {
    let doc = validation_audit();
    let parity = parity_audit();
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        for case in &EDIT_CASES {
            let row = audit_row(&doc, case.id);
            let message = edit_text(&row, case);
            let state = if case.label == FIELD_TRAN_TYPE_CODE {
                KEY_FLAG_STATE
            } else {
                DESCRIPTION_FLAG_STATE
            };
            assert_cursor_field(&parity, &row, state);
            eprintln!("{P_STORE_MAP}, {P_NUM_EDIT}, {P_ALPHANUM_EDIT}: {case:?}");
            let resp = send(&app, &token, case.request).await;
            assert_row_error(&resp, &row, &message);
        }
        for case in &PRECEDENCE_CASES {
            let answers = audit_row(&doc, case.answers.id);
            let loses = audit_row(&doc, case.loses);
            assert!(
                answers.order < loses.order,
                "{} (Order {}) must precede {} (Order {})",
                answers.id,
                answers.order,
                loses.id,
                loses.order
            );
            let message = edit_text(&answers, &case.answers);
            eprintln!("{P_KEY_EDITS}, {P_COMPARE}: {case:?}");
            let resp = send(&app, &token, case.answers.request).await;
            assert_row_error(&resp, &answers, &message);
        }
        for case in &OVER_LENGTH_CASES {
            let fields: Vec<(&str, &str)> = case
                .fields
                .iter()
                .map(|field| (field.as_str(), NO_MESSAGE))
                .collect();
            eprintln!("{case:?}");
            let resp = send(&app, &token, case.request).await;
            assert_error(
                &resp,
                StatusCode::BAD_REQUEST,
                ErrorKind::Invalid.code(),
                NO_MESSAGE,
                &fields,
            );
        }
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "a refused request writes nothing"
        );
    });
}

/// `POST` stores a new type in the master and not in the processing copy, its
/// key as the two digits `NUMVAL` gives and its description trimmed. A code
/// on file, seeded or just added, is refused 409 without a message before the
/// description is edited, and no row changes.
#[test]
fn an_add_writes_the_master_only_and_refuses_a_code_on_file() {
    let doc = validation_audit();
    let duplicate = audit_row(&doc, DUPLICATE);
    let invalid_description = audit_row(&doc, INVALID_DESCRIPTION_ON_ADD);
    assert!(
        duplicate.order < invalid_description.order,
        "{DUPLICATE} must precede {INVALID_DESCRIPTION_ON_ADD}"
    );
    let no_message = audited_text(&duplicate, &[]);
    let seeded = fixture_value(&parity_audit(), P_SEEDED_TYPE, "value");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        let copy = text_rows(&app, PROCESSING_COPY_ROWS).await;
        let added = [
            (add(Some("08"), GIFT_CARD), "08", "GIFT CARD"),
            (
                add(Some("9"), Some("  STORE CREDIT  ")),
                "09",
                "STORE CREDIT",
            ),
        ];
        let mut expected = master.clone();
        for (request, code, description) in added {
            eprintln!("{P_UPDATE_OUTCOMES}, {P_INSERT}, {P_KEY_EDITS}, {P_COMMITTED}: {request:?}");
            let resp = send(&app, &token, request).await;
            let message = CHANGES_COMMITTED_TO_DATABASE;
            assert_answer(&resp, StatusCode::CREATED, code, description, message);
            expected.push(type_row(code, description));
        }
        expected.sort();
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            expected,
            "the added types"
        );
        assert_eq!(
            text_rows(&app, PROCESSING_COPY_ROWS).await,
            copy,
            "an add reaches the Db2 master only"
        );

        let on_file = [
            add(Some(&seeded), GIFT_CARD),
            add(Some(&seeded), PURCHASE_RETAIL),
            add(Some("08"), Some("OTHER CARD")),
            add(Some("8"), Some("OTHER CARD")),
        ];
        for request in on_file {
            eprintln!("{DUPLICATE}, {P_ADD_ON_FILE}: {request:?}");
            let resp = send(&app, &token, request).await;
            assert_row_error(&resp, &duplicate, &no_message);
        }
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            expected,
            "{P_ADD_ON_FILE}: a refused add changes no row"
        );
    });
}

/// `PUT` without `origin`: a code with no type is 404 before the description
/// is edited; the stored values are no change; a change is committed to the
/// master with its description trimmed, while the processing copy keeps the
/// fixture description, and repeating the stored change is no change again.
#[test]
fn an_update_without_origin_writes_the_master_only() {
    let doc = validation_audit();
    let not_found = audit_row(&doc, NOT_FOUND);
    let blank_description = audit_row(&doc, BLANK_DESCRIPTION_ON_UPDATE);
    assert!(
        not_found.order < blank_description.order,
        "{NOT_FOUND} must precede {BLANK_DESCRIPTION_ON_UPDATE}"
    );
    let missing_text = NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE;
    assert_eq!(audited_text(&not_found, &[]), missing_text, "{NOT_FOUND}");
    let no_change = audit_row(&doc, NO_CHANGE);
    assert_eq!(no_change.code, None, "{NO_CHANGE} answers no error");
    let unchanged_text = NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED;
    assert_eq!(audited_text(&no_change, &[]), unchanged_text, "{NO_CHANGE}");
    let parity = parity_audit();
    assert_cursor_field(&parity, &not_found, KEY_NOT_FOUND_STATE);
    let code = fixture_value(&parity, P_SEEDED_TYPE, "value");
    let stored = fixture_value(&parity, P_SEEDED_TYPE, "tr_description");
    let missing = fixture_value(&parity, P_MISSING_TYPE, "value");
    let processing = fixture_value(&parity, P_PROCESSING_TYPE, "tran_type_desc");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        let copy = text_rows(&app, PROCESSING_COPY_ROWS).await;
        assert!(
            master.contains(&type_row(&code, &stored)),
            "{P_SEEDED_TYPE}"
        );
        assert!(
            copy.contains(&type_row(&code, &processing)),
            "{P_PROCESSING_TYPE}"
        );
        assert!(
            master.iter().all(|row| row.first() != Some(&missing)),
            "{P_MISSING_TYPE}"
        );

        for description in [Some("RETAIL PURCHASE"), Some("")] {
            eprintln!("{NOT_FOUND}, {P_READ}, {P_DECIDE}: {description:?}");
            let resp = send(&app, &token, update(&missing, description)).await;
            assert_row_error(&resp, &not_found, missing_text);
        }
        assert_no_change(&app, &token, &no_change, &code, &stored).await;
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{NO_CHANGE}: no change writes nothing"
        );

        let changed = "RETAIL PURCHASE";
        let resp = send(&app, &token, update(&code, Some("  RETAIL PURCHASE  "))).await;
        let committed = info_text(&parity, INFO_UPDATE_DONE);
        assert_eq!(committed, CHANGES_COMMITTED_TO_DATABASE, "{P_INFO_LINE}");
        assert_answer(&resp, StatusCode::OK, &code, changed, &committed);
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            with_description(&master, &code, changed),
            "{P_COMMITTED}: the master holds the trimmed description"
        );
        assert_eq!(
            text_rows(&app, PROCESSING_COPY_ROWS).await,
            copy,
            "trantype_file keeps the fixture description until TRANEXTR"
        );
        eprintln!("{P_FETCHED}: the stored change is fetched");
        let resp = send(&app, &token, update(&code, Some(changed))).await;
        assert_answer(&resp, no_change.status, &code, changed, unchanged_text);
    });
}

/// Asserts that the update of the seeded type `code`, whose description is
/// `stored`, finds no change, `row`, for the stored text, the same text in
/// lower case and padded with spaces, and the key `NUMVAL` reads as `code`:
/// the compare upper-cases and trims both texts, and the key is edited
/// before the read.
async fn assert_no_change(app: &TestApp, token: &str, row: &AuditRow, code: &str, stored: &str) {
    let lower = stored.to_lowercase();
    let padded = format!("  {stored}  ");
    let short_key = code.trim_start_matches('0');
    assert_ne!(
        short_key, code,
        "{P_SEEDED_TYPE} has a leading zero to drop"
    );
    let inputs = [
        (code, stored),
        (code, lower.as_str()),
        (code, padded.as_str()),
        (short_key, stored),
    ];
    for (key, description) in inputs {
        eprintln!("{NO_CHANGE}, {P_COMPARE}, {P_KEY_EDITS}: {key:?} {description:?}");
        let resp = send(app, token, update(key, Some(description))).await;
        let message = NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED;
        assert_answer(&resp, row.status, code, stored, message);
    }
}

/// Each read, write and delete failure of the three operations, injected on
/// the master, is answered with the Db2 text of its row, on the field the
/// cursor is put on in the state it leaves, and writes nothing.
#[test]
fn read_and_write_failures_answer_their_db2_texts() {
    let doc = validation_audit();
    let parity = parity_audit();
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        app.stack().exec_sql(FAULT_FUNCTIONS).await;
        let master = text_rows(&app, MASTER_ROWS).await;
        for case in &FAILURE_CASES {
            let row = audit_row(&doc, case.id);
            let message = failure_text(&row, case.text);
            assert_cursor_field(&parity, &row, case.cursor);
            eprintln!("{P_READ}, {P_DECIDE}, {P_UPDATE_OUTCOMES}, {P_INSERT}: {case:?}");
            app.stack().exec_sql(case.fault.inject()).await;
            let resp = send(&app, &token, case.request).await;
            app.stack().exec_sql(case.fault.restore()).await;
            assert_row_error(&resp, &row, &message);
            assert_eq!(
                text_rows(&app, MASTER_ROWS).await,
                master,
                "{}: a failed request writes nothing",
                case.id
            );
        }
    });
}

/// The `code` of the API's answer to a method a route does not serve.
const METHOD_NOT_ALLOWED_CODE: &str = "METHOD_NOT_ALLOWED";

/// The parsed `docs/parity/function-map.md` (`F-` rows).
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The methods the function map names for each route of the `COTRTUP`
/// controls: the `Endpoint` cell of every row of the map that sends a
/// request, its query string dropped. Navigation and the rows without a
/// control (`—`) name none.
fn mapped_methods(doc: &Document) -> BTreeMap<String, BTreeSet<String>> {
    let mut methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for table in doc.tables().iter().filter(|table| table.is_data_table()) {
        for row in &table.rows {
            if table.cell(row, "BMS map").map(str::trim) != Some(COTRTUP_MAP) {
                continue;
            }
            let endpoint = table.cell(row, "Endpoint").unwrap_or_default().trim();
            let Some((method, target)) = endpoint.split_once(' ') else {
                continue;
            };
            if method.is_empty() || !method.bytes().all(|byte| byte.is_ascii_uppercase()) {
                continue;
            }
            let route = target.split('?').next().unwrap_or(target);
            methods
                .entry(route.to_owned())
                .or_default()
                .insert(method.to_owned());
        }
    }
    methods
}

/// The entries of the menu source `menu` that hold `needle`: each text
/// between an opening brace and the next closing one.
fn menu_entries<'a>(menu: &'a str, needle: &str) -> Vec<&'a str> {
    menu.split('{')
        .filter_map(|part| part.split_once('}').map(|(entry, _)| entry))
        .filter(|entry| entry.contains(needle))
        .collect()
}

/// The failure case of the update (`of_update`) or of the add that meets
/// `fault`.
fn failure_case(fault: Fault, of_update: bool) -> &'static FailureCase {
    FAILURE_CASES
        .iter()
        .find(|case| {
            case.fault == fault && matches!(case.request, Request::Update { .. }) == of_update
        })
        .unwrap_or_else(|| panic!("no failure case meets {fault:?} (update: {of_update})"))
}

/// The seeded master row of `code`.
fn seeded_row<'a>(master: &'a [Vec<String>], code: &str) -> &'a [String] {
    master
        .iter()
        .find(|row| row.first().map(String::as_str) == Some(code))
        .unwrap_or_else(|| panic!("the master holds no type {code}: {master:?}"))
}

/// The `items` a look-up answers when it shows the one type `code` with
/// `description`.
fn shown_items(code: &str, description: &str) -> Value {
    json(&[TranTypeResponse {
        type_code: code.to_owned(),
        description: description.to_owned(),
    }])
}

/// The member names of the JSON object `value`, sorted; none when it is not
/// an object.
fn members(value: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = value
        .as_object()
        .map(|object| object.keys().map(String::as_str).collect())
        .unwrap_or_default();
    names.sort_unstable();
    names
}

/// Sends `builder` with the bearer token `token`: the answer's
/// `Content-Type`, empty when absent, and the answer.
async fn send_reading_media_type(builder: RequestBuilder, token: &str) -> (String, Resp) {
    let response = builder
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("the test request failed: {error}"));
    let media_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let status = response.status();
    let raw = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("reading the test response body failed: {error}"));
    let body = serde_json::from_str(&raw).unwrap_or(Value::Null);
    (media_type, (status, body, raw))
}

/// F3 sends nothing: its function map row names no request, and its `Back`
/// target is the route of the type list, the calling program, which the SPA
/// navigation lists in the section of the admin menu `COADM01C`, the other
/// program F3 returns to. F3's `SYNCPOINT` has nothing left to commit: every
/// write is committed when it answers
/// (`the_add_and_the_delete_are_one_request_each`). Needs no Docker.
#[test]
fn the_exit_key_returns_to_the_list_route_of_the_admin_menu() {
    let doc = function_map();
    let (table, row) = doc
        .find_row(F_BACK)
        .unwrap_or_else(|| panic!("the function map has no {F_BACK} row"));
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{F_BACK} has no `{name}` cell"))
            .trim()
    };
    assert_eq!(cell("BMS map"), COTRTUP_MAP, "{F_BACK}");
    assert_eq!(
        cell("Endpoint"),
        NAVIGATION_ENDPOINT,
        "{P_EXIT}: F3 sends no request"
    );
    let target = key_values(cell("UI control"))
        .into_iter()
        .find(|pair| pair.key == "target")
        .unwrap_or_else(|| panic!("{F_BACK} names no target"))
        .value;
    let path = fixtures::repo_root().join(MENU_FILE);
    let menu = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let entries = menu_entries(&menu, LIST_PROGRAM_ENTRY);
    let [entry] = entries.as_slice() else {
        panic!("{MENU_FILE} must hold one type list entry: {entries:?}");
    };
    let route = format!("route: \"{target}\",");
    assert!(
        entry.contains(&route),
        "{P_EXIT}: the Back target {target} is the type list route: {entry}"
    );
    assert!(
        entry.contains(ADMIN_SECTION_ENTRY),
        "{P_EXIT}: the type list is an option of the admin menu: {entry}"
    );
}

/// The attention keys of `COTRTUP` are the methods its function map rows
/// name, on the routes they name. Every other method is 405 with the API's
/// method text, never the invalid-key text, which E-56 does not reproduce. A
/// body member naming a key is not read: the update answers as for ENTER,
/// whatever key the member names. Nothing is written.
#[test]
fn the_keys_are_the_mapped_methods_and_no_member_is_read_as_a_key() {
    let mapped = mapped_methods(&function_map());
    let routes: Vec<&str> = mapped.keys().map(String::as_str).collect();
    assert_eq!(
        routes,
        [TYPES_PATH, TYPE_PATH_TEMPLATE],
        "{P_KEYS}: the routes of the COTRTUP controls"
    );
    let no_change = audit_row(&validation_audit(), NO_CHANGE);
    let parity = parity_audit();
    let code = fixture_value(&parity, P_SEEDED_TYPE, "value");
    let stored = fixture_value(&parity, P_SEEDED_TYPE, "tr_description");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        for (route, served) in &mapped {
            let path = route.replace("{id}", &code);
            for method in METHODS {
                let builder = app
                    .client()
                    .request(method.clone(), app.url(&path))
                    .json(&Value::Object(Map::new()));
                let resp = common::send(builder, Some(&token)).await;
                if served.contains(method.as_str()) {
                    assert_ne!(
                        resp.0,
                        StatusCode::METHOD_NOT_ALLOWED,
                        "{P_KEYS}: {method} {path} is a key of the map: {}",
                        resp.2
                    );
                } else {
                    eprintln!("{P_KEYS}: {method} {path} is no key of the map");
                    let status = StatusCode::METHOD_NOT_ALLOWED;
                    let code = METHOD_NOT_ALLOWED_CODE;
                    assert_error(&resp, status, code, METHOD_NOT_ALLOWED, &[]);
                }
            }
        }
        for key in AID_KEYS {
            let mut body = json(&TranTypeUpdateRequest {
                description: Some(stored.clone()),
            });
            body.as_object_mut()
                .unwrap_or_else(|| panic!("the update body is an object"))
                .insert(AID_MEMBER.to_owned(), Value::from(key));
            eprintln!("{P_KEYS}: {AID_MEMBER}={key}");
            let resp = app.put(&type_path(&code), Some(&token), &body).await;
            let message = NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED;
            assert_answer(&resp, no_change.status, &code, &stored, message);
        }
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_KEYS}: no key wrote"
        );
    });
}

/// F5 on a key not found, which the F6 legend also stands for, is one
/// `POST`; F4 confirmed is one `DELETE`; a validated change is written by the
/// one `PUT` that validates it. The look-up that shows the type writes
/// nothing, so the delete confirmation the SPA asks, and its cancel, send no
/// request. Every write is committed when it answers, as F3's `SYNCPOINT`
/// commits. The delete, `origin=form`, answers `Delete successful.`, the
/// text of `9800-DELETE-PROCESSING` and of the delete-done info line, with
/// the type as read, which the delete-done screen still shows.
#[test]
fn the_add_and_the_delete_are_one_request_each() {
    let parity = parity_audit();
    let deleted = fixture_value(&parity, P_DELETED, "info");
    assert_eq!(deleted, DELETE_SUCCESSFUL, "{P_DELETED}");
    assert_eq!(
        info_text(&parity, INFO_DELETE_DONE),
        deleted,
        "{P_INFO_LINE}"
    );
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        assert!(
            master
                .iter()
                .all(|row| row.first().map(String::as_str) != Some(NEW_CODE)),
            "{NEW_CODE} is not seeded"
        );
        let (status, _, raw) = app.get(NEW_CODE_LOOKUP, Some(&token)).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{P_ADD_KEY}: the look-up finds no type {NEW_CODE}: {raw}"
        );
        let resp = send(&app, &token, add(Some(NEW_CODE), GIFT_CARD)).await;
        let committed = CHANGES_COMMITTED_TO_DATABASE;
        assert_answer(&resp, StatusCode::CREATED, NEW_CODE, "GIFT CARD", committed);
        let mut added = master.clone();
        added.push(type_row(NEW_CODE, "GIFT CARD"));
        added.sort();
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            added,
            "{P_ADD_KEY}, {P_DELETE_AND_ADD}, {P_EXIT}: the add is committed when it answers"
        );

        let (status, body, raw) = app.get(NEW_CODE_LOOKUP, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "the look-up shows the type: {raw}");
        let shown = shown_items(NEW_CODE, "GIFT CARD");
        assert_eq!(body["items"], shown, "the look-up shows the type: {raw}");
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            added,
            "{P_ASK_DELETE}, {P_CANCEL}: showing the type, and asking or cancelling its delete, \
             writes nothing"
        );

        let resp = send(&app, &token, update(NEW_CODE, Some("STORE CREDIT"))).await;
        assert_answer(&resp, StatusCode::OK, NEW_CODE, "STORE CREDIT", committed);
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            with_description(&added, NEW_CODE, "STORE CREDIT"),
            "{P_CANCEL}: the change is validated and written by one request"
        );

        eprintln!(
            "{P_ASK_DELETE}, {P_DELETE_AND_ADD}, {P_DELETED}: the confirmed F4 is one DELETE"
        );
        let resp = send(&app, &token, delete(NEW_CODE)).await;
        assert_answer(&resp, StatusCode::OK, NEW_CODE, "STORE CREDIT", &deleted);
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_DELETE_AND_ADD}, {P_EXIT}: the delete is committed when it answers"
        );
    });
}

/// The F4 delete, `origin=form`, answers the `COTRTUPC` texts on the key: a
/// code with no type is 404 before anything is deleted; categories that
/// reference the type refuse its delete with the -532 text, and a delete that
/// fails answers the other text, each composed as `9800-DELETE-PROCESSING`
/// strings it; the type stays. Without `origin` the same `DELETE` keeps the
/// `COTRTLIC` texts (E-23).
#[test]
fn the_form_delete_answers_the_cotrtupc_texts() {
    let doc = validation_audit();
    let parity = parity_audit();
    let not_found = audit_row(&doc, NOT_FOUND_ON_DELETE);
    let missing_text = NO_RECORD_FOUND_FOR_THIS_KEY_IN_DATABASE;
    assert_eq!(
        audited_text(&not_found, &[]),
        missing_text,
        "{NOT_FOUND_ON_DELETE}"
    );
    assert_cursor_field(&parity, &not_found, KEY_NOT_FOUND_STATE);
    let restricted_row = audit_row(&doc, RESTRICTED_DELETE);
    let child_records = TranTypeSqlFailure::DeleteChildRecords;
    let restricted = delete_failure_text(&parity, child_records, Condition::Restricted.sqlcode());
    assert_eq!(
        failure_text(
            &restricted_row,
            Db2Text::Sql(child_records, Condition::Restricted)
        ),
        restricted,
        "{P_DELETE_FAILURES}: the -532 text of {RESTRICTED_DELETE}"
    );
    let lock = FAILURE_CASES
        .iter()
        .find(|case| case.fault == Fault::LockOnDelete)
        .unwrap_or_else(|| panic!("no failure case meets {:?}", Fault::LockOnDelete));
    let failed_row = audit_row(&doc, lock.id);
    let failed = delete_failure_text(
        &parity,
        TranTypeSqlFailure::Delete,
        Condition::Contention.sqlcode(),
    );
    assert_eq!(
        failure_text(&failed_row, lock.text),
        failed,
        "{P_DELETE_FAILURES}: the other failure's text of {}",
        lock.id
    );
    let missing = fixture_value(&parity, P_MISSING_TYPE, "value");
    let code = fixture_value(&parity, P_SEEDED_TYPE, "value");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        app.stack().exec_sql(FAULT_FUNCTIONS).await;
        let master = text_rows(&app, MASTER_ROWS).await;
        eprintln!("{NOT_FOUND_ON_DELETE}, {P_READ}: the F4 delete of {missing}");
        let resp = send(&app, &token, delete(&missing)).await;
        assert_row_error(&resp, &not_found, missing_text);
        eprintln!("{P_DELETE_FAILURES}: the F4 delete of {code}, which categories reference");
        let resp = send(&app, &token, delete(&code)).await;
        assert_row_error(&resp, &restricted_row, &restricted);
        eprintln!("{P_DELETE_FAILURES}: {lock:?}");
        app.stack().exec_sql(lock.fault.inject()).await;
        let resp = send(&app, &token, lock.request).await;
        app.stack().exec_sql(lock.fault.restore()).await;
        assert_row_error(&resp, &failed_row, &failed);
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_DELETE_FAILURES}: a refused or failed delete keeps the type"
        );
        assert_list_delete_keeps_its_texts(&app, &token, &parity, &code, &restricted).await;
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_DELETED}: the list delete removed only the type it added"
        );
    });
}

/// Asserts that `DELETE` without `origin` is the `COTRTLIC` row delete
/// (E-23): the type `code`, which categories reference, is refused 409 with
/// a text other than the `COTRTUPC` -532 text `restricted`; and a type added
/// for the purpose is deleted with the `COTRTLIC` text [`P_DELETED`] names
/// for the same delete, and no type.
async fn assert_list_delete_keeps_its_texts(
    app: &TestApp,
    token: &str,
    parity: &Document,
    code: &str,
    restricted: &str,
) {
    let info = key_values(expected_result(parity, P_DELETED))
        .into_iter()
        .find(|pair| pair.key == "info")
        .unwrap_or_else(|| panic!("{P_DELETED} gives no `info`"))
        .value;
    let spans = cell_code_spans(&info);
    let Some(listed) = spans.get(1) else {
        panic!("{P_DELETED} quotes no COTRTLIC text: {info}");
    };
    let listed_deleted = listed.text.trim_matches('"');
    let (status, body, raw) = app.delete(&type_path(code), Some(token)).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some(ErrorKind::Conflict.code())),
        "{P_DELETE_FAILURES}: the list delete of {code}: {raw}"
    );
    assert_ne!(
        body["message"].as_str(),
        Some(restricted),
        "{P_DELETE_FAILURES}: the list delete keeps the COTRTLIC -532 text: {raw}"
    );
    let resp = send(app, token, add(Some(LISTED_NEW_CODE), GIFT_CARD)).await;
    let committed = CHANGES_COMMITTED_TO_DATABASE;
    assert_answer(
        &resp,
        StatusCode::CREATED,
        LISTED_NEW_CODE,
        "GIFT CARD",
        committed,
    );
    let (status, body, raw) = app.delete(&type_path(LISTED_NEW_CODE), Some(token)).await;
    assert_eq!(status, StatusCode::OK, "{P_DELETED}: {raw}");
    assert_eq!(
        body,
        serde_json::json!({"transaction_type": null, "message": listed_deleted}),
        "{P_DELETED}: the list delete answers the COTRTLIC text and no type: {raw}"
    );
}

/// The map is stored and edited before it is read: with the master taken
/// away, so that any read fails whatever code it addresses, each key edit of
/// the update answers its row, and only a valid key reaches the read. That
/// read failure has no Db2 code, so it is answered as the `Unexpected` kind,
/// on the update and on the add. Nothing is written.
#[test]
fn the_edits_run_before_the_read_and_the_read_before_the_write() {
    let doc = validation_audit();
    let key_edits: Vec<&EditCase> = EDIT_CASES
        .iter()
        .filter(|case| {
            case.label == FIELD_TRAN_TYPE_CODE && matches!(case.request, Request::Update { .. })
        })
        .collect();
    let rows: BTreeSet<&str> = key_edits.iter().map(|case| case.id).collect();
    assert_eq!(
        rows.len(),
        3,
        "the three key edit rows of the update: {rows:?}"
    );
    let unread = [
        update("01", Some("UNREAD MASTER")),
        add(Some("01"), Some("UNREAD MASTER")),
    ];
    let unexpected_fields = [(TranTypeUpsertField::TrTypCd.as_str(), UNEXPECTED_ERROR)];
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        let fault = Fault::MasterMissing;
        app.stack().exec_sql(fault.inject()).await;
        let mut edited = Vec::new();
        for case in &key_edits {
            edited.push(send(&app, &token, case.request).await);
        }
        let mut read = Vec::new();
        for request in unread {
            read.push(send(&app, &token, request).await);
        }
        app.stack().exec_sql(fault.restore()).await;
        for (case, resp) in key_edits.iter().zip(&edited) {
            let row = audit_row(&doc, case.id);
            let message = edit_text(&row, case);
            eprintln!("{P_PROCESS_INPUTS}, {P_DECIDE}: {case:?}, the master away");
            assert_row_error(resp, &row, &message);
        }
        for (request, resp) in unread.iter().zip(&read) {
            eprintln!("{P_PROCESS_INPUTS}, {P_ABEND}: {request:?}, the master away");
            let code = ErrorKind::Unexpected.code();
            let status = StatusCode::INTERNAL_SERVER_ERROR;
            assert_error(resp, status, code, UNEXPECTED_ERROR, &unexpected_fields);
        }
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_PROCESS_INPUTS}: no edit or failed read writes"
        );
    });
}

/// No request leaves a state the next one meets. The update that met a lock
/// is repeated and committed with its own text only. An F4 delete refused
/// because categories reference the type (SQLCODE -532) is 409 with the
/// `COTRTUPC` child-records text and keeps the type, whose `TTUP-START-DELETE`
/// the COBOL keeps (P-COTRTUPC-005, E-69); ENTER with the stored values then
/// finds no change, F5 with a change commits it, and the add of the code is
/// refused as for any code on file: no request meets the abend the COBOL
/// reaches from that state.
#[test]
fn no_request_leaves_a_state_for_the_next() {
    let doc = validation_audit();
    let lock = failure_case(Fault::LockOnUpdate, true);
    let lock_row = audit_row(&doc, lock.id);
    let no_change = audit_row(&doc, NO_CHANGE);
    let duplicate = audit_row(&doc, DUPLICATE);
    let restricted_row = audit_row(&doc, RESTRICTED_DELETE);
    let restricted_text = Db2Text::Sql(
        TranTypeSqlFailure::DeleteChildRecords,
        Condition::Restricted,
    );
    let restricted = failure_text(&restricted_row, restricted_text);
    let parity = parity_audit();
    assert_cursor_field(&parity, &restricted_row, DELETE_STATE);
    let code = fixture_value(&parity, P_SEEDED_TYPE, "value");
    let stored = fixture_value(&parity, P_SEEDED_TYPE, "tr_description");
    let change = "RETAIL PURCHASE";
    let committed = CHANGES_COMMITTED_TO_DATABASE;
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        app.stack().exec_sql(FAULT_FUNCTIONS).await;
        let master = text_rows(&app, MASTER_ROWS).await;
        app.stack().exec_sql(lock.fault.inject()).await;
        let resp = send(&app, &token, update(&code, Some(change))).await;
        app.stack().exec_sql(lock.fault.restore()).await;
        assert_row_error(&resp, &lock_row, &failure_text(&lock_row, lock.text));
        eprintln!("{P_RESET}, {P_COMMAREA}: the update after its lock failure");
        let resp = send(&app, &token, update(&code, Some(change))).await;
        assert_answer(&resp, StatusCode::OK, &code, change, committed);
        let changed = with_description(&master, &code, change);

        eprintln!("{P_RESTRICTED_DELETE}: the categories of {code} refuse its delete");
        let resp = send(&app, &token, delete(&code)).await;
        assert_row_error(&resp, &restricted_row, &restricted);
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            changed,
            "{P_RESTRICTED_DELETE}: the type stays"
        );

        eprintln!("{P_DECIDE}, {P_ABEND}, {P_RESTRICTED_DELETE}: ENTER, F5 and the add after it");
        let resp = send(&app, &token, update(&code, Some(change))).await;
        let unchanged = NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED;
        assert_answer(&resp, no_change.status, &code, change, unchanged);
        let resp = send(&app, &token, update(&code, Some(&stored))).await;
        assert_answer(&resp, StatusCode::OK, &code, &stored, committed);
        let resp = send(&app, &token, add(Some(&code), GIFT_CARD)).await;
        assert_row_error(&resp, &duplicate, &audited_text(&duplicate, &[]));
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            master,
            "{P_RESET}: each request after the refused delete is a fresh entry"
        );
    });
}

/// Each request carries the key it is for, and the server keeps nothing of
/// the requests before it. The type the client showed is changed by another
/// unit of work before the client's update, which `COTRTUPC` never checks,
/// as it never sets `DATA-WAS-CHANGED-BEFORE-UPDATE`; the update of another
/// type in between leaves nothing behind: the client's update is committed.
#[test]
fn each_request_carries_its_own_key() {
    let code = fixture_value(&parity_audit(), P_SEEDED_TYPE, "value");
    let change = "RETAIL PURCHASE";
    let committed = CHANGES_COMMITTED_TO_DATABASE;
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let master = text_rows(&app, MASTER_ROWS).await;
        let [_, seeded] = seeded_row(&master, OTHER_CODE) else {
            panic!("a master row is a code and a description");
        };
        let (status, body, raw) = app.get(OTHER_CODE_LOOKUP, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "the look-up shows the type: {raw}");
        let shown = shown_items(OTHER_CODE, seeded);
        assert_eq!(body["items"], shown, "the look-up shows the type: {raw}");
        app.stack().exec_sql(CHANGE_OTHER_ELSEWHERE).await;

        eprintln!("{P_COMMAREA}: the update of another type in between");
        let resp = send(&app, &token, update(&code, Some(change))).await;
        assert_answer(&resp, StatusCode::OK, &code, change, committed);
        eprintln!("{P_COMMAREA}, {P_UNREACHED}: the update of the type changed since shown");
        let resp = send(&app, &token, update(OTHER_CODE, GIFT_CARD)).await;
        assert_answer(&resp, StatusCode::OK, OTHER_CODE, "GIFT CARD", committed);
        let expected = with_description(
            &with_description(&master, &code, change),
            OTHER_CODE,
            "GIFT CARD",
        );
        assert_eq!(
            text_rows(&app, MASTER_ROWS).await,
            expected,
            "{P_COMMAREA}, {P_UNREACHED}: both updates are committed"
        );
    });
}

/// An answer is the JSON body of its DTO, with none of the screen's header,
/// date or time, which are free in the SPA. A success answer shows the
/// values the screen shows in its state: the fetched values when no change
/// is found, the stored new values once an update or an add is committed,
/// and the old values once a delete is done. An error answer shows none: the
/// form keeps what was entered.
#[test]
fn an_answer_is_its_json_dto_with_the_values_of_its_state() {
    let no_change = audit_row(&validation_audit(), NO_CHANGE);
    let parity = parity_audit();
    let code = fixture_value(&parity, P_SEEDED_TYPE, "value");
    let stored = fixture_value(&parity, P_SEEDED_TYPE, "tr_description");
    let missing = fixture_value(&parity, P_MISSING_TYPE, "value");
    let lower = stored.to_lowercase();
    let unchanged = NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED;
    let committed = CHANGES_COMMITTED_TO_DATABASE;
    let successes = [
        (
            update(&code, Some(&lower)),
            no_change.status,
            code.as_str(),
            stored.as_str(),
            unchanged,
        ),
        (
            update(&code, Some("  retail purchase  ")),
            StatusCode::OK,
            code.as_str(),
            "retail purchase",
            committed,
        ),
        (
            add(Some(NEW_CODE), GIFT_CARD),
            StatusCode::CREATED,
            NEW_CODE,
            "GIFT CARD",
            committed,
        ),
        // The delete-done screen shows the old values with its text.
        (
            delete(NEW_CODE),
            StatusCode::OK,
            NEW_CODE,
            "GIFT CARD",
            DELETE_SUCCESSFUL,
        ),
    ];
    let errors = [update(&missing, GIFT_CARD), update(&code, PURCHASE_RETAIL)];
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        for (request, status, shown_code, shown_description, message) in successes {
            let builder = request_builder(&app, request);
            let (media_type, resp) = send_reading_media_type(builder, &token).await;
            let raw = &resp.2;
            assert!(
                media_type.starts_with(JSON_MEDIA_TYPE),
                "{P_SEND_MAP}: {media_type}"
            );
            assert_eq!(members(&resp.1), ANSWER_MEMBERS, "{P_SEND_MAP}: {raw}");
            let shown = &resp.1["transaction_type"];
            assert_eq!(members(shown), TYPE_MEMBERS, "{P_SEND_MAP}: {raw}");
            eprintln!("{P_SHOWN_VALUES}: {request:?}");
            assert_answer(&resp, status, shown_code, shown_description, message);
        }
        for request in errors {
            let builder = request_builder(&app, request);
            let (media_type, resp) = send_reading_media_type(builder, &token).await;
            assert!(
                media_type.starts_with(JSON_MEDIA_TYPE),
                "{P_SEND_MAP}: {media_type}"
            );
            assert_eq!(
                members(&resp.1),
                ERROR_MEMBERS,
                "{P_SEND_MAP}, {P_SHOWN_VALUES}: an error answer shows no values: {}",
                resp.2
            );
        }
    });
}

/// `3390-SETUP-INFOMSG-ATTRS` and `3391-SETUP-PFKEY-ATTRS`: the legend and the
/// attributes that brighten or darken its keys and the info line by state.
const P_LEGEND: &str = "P-COTRTUPC-026";

/// The `COTRTUP` map source, whose `INITIAL` texts are the legend.
const COTRTUP_BMS: &str = "app/app-transaction-type-db2/bms/COTRTUP.bms";

/// What [`P_LEGEND`] records: the legend texts its expected result quotes,
/// the stems of the attribute fields it sets (`FKEY04A` gives `FKEY04`), and
/// the function map rows its `replacement=` names.
struct Legend {
    texts: Vec<String>,
    stems: Vec<String>,
    named: BTreeSet<String>,
}

impl Legend {
    /// The legend of [`P_LEGEND`], once its last review verdict is `pass`.
    fn read(parity: &Document) -> Self {
        let verdicts: Vec<&str> = parity
            .tables()
            .iter()
            .filter(|table| table.is_review_log())
            .flat_map(|table| {
                table
                    .rows
                    .iter()
                    .filter(|row| table.cell(row, "Row ID") == Some(P_LEGEND))
                    .filter_map(|row| table.cell(row, "Verdict"))
            })
            .collect();
        assert_eq!(
            verdicts.last(),
            Some(&"pass"),
            "review verdicts of {P_LEGEND}"
        );
        let (table, row) = parity
            .find_row(P_LEGEND)
            .unwrap_or_else(|| panic!("the parity audit has no {P_LEGEND} row"));
        let cell = |name: &str| {
            table
                .cell(row, name)
                .unwrap_or_else(|| panic!("{P_LEGEND} has no `{name}` cell"))
        };
        let expected = cell("Expected result");
        let texts = carddemo_audit::cell_code_spans(expected)
            .iter()
            .map(|span| {
                let quoted = span.text.as_str();
                let text = quoted.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
                text.unwrap_or_else(|| panic!("{P_LEGEND} quotes {quoted:?}"))
                    .to_owned()
            })
            .collect();
        let stems = cell("Copybook field or PIC clause")
            .split(", ")
            .map(|field| {
                let stem = field.trim().strip_suffix('A');
                stem.unwrap_or_else(|| panic!("{P_LEGEND}: {field} is no attribute field"))
                    .to_owned()
            })
            .collect();
        let (_, replacement) = expected
            .split_once("replacement=")
            .unwrap_or_else(|| panic!("{P_LEGEND} names no replacement="));
        let named = carddemo_audit::id_tokens(replacement)
            .into_iter()
            .filter(|(kind, _)| *kind == carddemo_audit::IdKind::Function)
            .map(|(_, id)| id)
            .collect();
        Self {
            texts,
            stems,
            named,
        }
    }
}

/// The `COTRTUP` function map rows with a control whose key cell cites the
/// `COTRTUP.bms` line holding the `INITIAL` of a legend text, each legend text
/// being cited by at least one.
fn legend_control_rows(legend: &Legend, map_doc: &Document) -> BTreeSet<String> {
    let path = carddemo_testkit::fixtures::repo_root().join(COTRTUP_BMS);
    let bms = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    let mut rows = BTreeSet::new();
    for text in &legend.texts {
        let initial = format!("INITIAL='{text}'");
        let line = bms
            .lines()
            .position(|source| source.contains(&initial))
            .map_or_else(|| panic!("{COTRTUP_BMS} holds no {initial}"), |at| at + 1);
        let citation = format!("[{COTRTUP_BMS}:L{line}]");
        let cited: Vec<String> = map_doc
            .tables()
            .iter()
            .filter(|table| table.is_data_table())
            .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
            .filter(|(table, row)| {
                let value = |name: &str| table.cell(row, name).unwrap_or_default();
                value("BMS map").trim() == COTRTUP_MAP
                    && value("Key or field").contains(&citation)
                    && key_values(value("UI control"))
                        .iter()
                        .any(|pair| pair.key == "control" && !pair.value.is_empty())
            })
            .map(|(table, row)| table.cell(row, "ID").unwrap_or_default().trim().to_owned())
            .collect();
        assert!(
            !cited.is_empty(),
            "{P_LEGEND}: no function map control stands for {text} ({citation})"
        );
        rows.extend(cited);
    }
    rows
}

/// The member names of `value` and its nested values, and its strings: every
/// string when `all_strings`, else only those of `enum` lists, the strings a
/// schema of the published document lets an answer carry.
fn json_words(value: &Value, all_strings: bool, words: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            for (key, member) in object {
                words.push(key.clone());
                json_words(member, all_strings || key == "enum", words);
            }
        }
        Value::Array(items) => {
            for item in items {
                json_words(item, all_strings, words);
            }
        }
        Value::String(text) if all_strings => words.push(text.clone()),
        _ => {}
    }
}

/// Asserts that no word of `value` (see [`json_words`]) holds a legend text
/// or names an attribute field of `legend`, whatever its case.
fn assert_no_legend(legend: &Legend, value: &Value, all_strings: bool, what: &str) {
    let mut words = Vec::new();
    json_words(value, all_strings, &mut words);
    for word in words {
        let lower = word.to_ascii_lowercase();
        for marker in legend.texts.iter().chain(&legend.stems) {
            assert!(
                !lower.contains(&marker.to_ascii_lowercase()),
                "{P_LEGEND}: {what} carries {marker:?} in {word:?}"
            );
        }
    }
}

/// The legend is presentation: each of its keys is the control that the
/// function map rows citing its `COTRTUP.bms` field give on the route of the
/// state, and those rows are exactly the ones [`P_LEGEND`]'s `replacement=`
/// names. No member or enumerated value of the published document, and no
/// member or string of a CTTU answer, carries a legend text or names an
/// attribute field: the look-up of a type found and of one not found, an
/// update unchanged, committed and refused, an add committed and refused, and
/// the F4 delete.
#[test]
fn the_legend_is_the_mapped_controls_and_no_answer_carries_it() {
    let legend = Legend::read(&parity_audit());
    assert_eq!(legend.texts.len(), 5, "{P_LEGEND}: the five legend fields");
    assert_eq!(
        legend_control_rows(&legend, &function_map()),
        legend.named,
        "{P_LEGEND}: the replacement names the controls of the legend keys"
    );
    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    assert_no_legend(&legend, &spec, false, "the OpenAPI document");
    let seeded = format!("{TYPES_PATH}/01");
    let unchanged = serde_json::json!({ "description": "PURCHASE" });
    let changed = serde_json::json!({ "description": "GIFT CARD" });
    let refused = serde_json::json!({ "description": "PURCHASE-RETAIL" });
    let added = serde_json::json!({ "type_code": "08", "description": "GIFT CARD" });
    let requests = [
        (Method::GET, format!("{TYPES_PATH}?type_code=01"), None),
        (Method::GET, format!("{TYPES_PATH}?type_code=99"), None),
        (Method::PUT, seeded.clone(), Some(unchanged)),
        (Method::PUT, seeded.clone(), Some(changed)),
        (Method::PUT, seeded, Some(refused)),
        (Method::POST, TYPES_PATH.to_owned(), Some(added.clone())),
        (Method::POST, TYPES_PATH.to_owned(), Some(added)),
        (Method::DELETE, format!("{TYPES_PATH}/08?origin=form"), None),
    ];
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        for (method, path, body) in requests {
            let mut builder = app.client().request(method.clone(), app.url(&path));
            if let Some(body) = body {
                builder = builder.json(&body);
            }
            let (status, body, raw) = common::send(builder, Some(&token)).await;
            assert!(body.is_object(), "{method} {path}: no JSON answer: {raw}");
            let what = format!("the {status} answer of {method} {path}");
            assert_no_legend(&legend, &body, true, &what);
        }
    });
}
