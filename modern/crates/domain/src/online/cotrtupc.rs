//! COTRTUPC, transaction-type maintenance: the rules of endpoints 26 and 27,
//! `GET /api/v1/transaction-types/{type_code}` and
//! `POST /api/v1/transaction-types/{type_code}/actions` (transaction `CTTU`, map `CTRTUPA`,
//! admin only; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COTRTUPC.md` over the Db2 data
//! version of the transaction types, `db2.transaction_type`, whose row is
//! [`Db2TransactionType`]. Only that table is read or written, never the VSAM version that
//! `tranrept` reads (COTRTUPC-002, E23). The rows ported are the input store (COTRTUPC-004),
//! the key edits (COTRTUPC-005 to COTRTUPC-008), the fetch (COTRTUPC-009 to COTRTUPC-012), the
//! ENTER edits (COTRTUPC-016 to COTRTUPC-021), the save (COTRTUPC-023 to COTRTUPC-029), the
//! delete (COTRTUPC-032 to COTRTUPC-034), the unexpected failure (COTRTUPC-038), the served keys
//! (COTRTUPC-043) and the halted Db2 text (COTRTUPC-049). The admin gate (COTRTUPC-001) is the
//! API's extractor. F3, F12, the first F4, F5 in state X, ENTER in state N, the abend screen, the
//! info row's layout and every attribute are `client` rows the screen implements (COTRTUPC-003,
//! COTRTUPC-013 to COTRTUPC-015, COTRTUPC-022, COTRTUPC-030, COTRTUPC-031, COTRTUPC-035 to
//! COTRTUPC-037, COTRTUPC-039 to COTRTUPC-042, COTRTUPC-044 to COTRTUPC-048).
//!
//! # State and the endpoint mapping
//!
//! COTRTUPC is pseudo-conversational: its COMMAREA keeps `TTUP-CHANGE-ACTION`, the fetched
//! values and the entered values between screens. The requests carry none of them
//! (`TranTypeActionRequest { aid, description? }`), and the server keeps no screen state: the
//! screen keeps the state, as the COMMAREA did. The state a request is served in is derived on
//! the server and never taken from the client. [`served_state`] maps the `aid`, and the row the
//! service re-reads for `{type_code}` as the stand-in for `TTUP-OLD-DETAILS`, to the state of the
//! audit's endpoint mapping:
//!
//! | `aid` | Row re-read | State | Step |
//! |---|---|---|---|
//! | ENTER | found | S | key edits, comparison with the row, description edits (COTRTUPC-016, COTRTUPC-017) |
//! | ENTER | none | R | no key edits, comparison with blanks (COTRTUPC-021) |
//! | PF5 | either | N | save: update, then insert when no row changed (COTRTUPC-023 to COTRTUPC-029) |
//! | PF4 | either | 9 | delete (COTRTUPC-032 to COTRTUPC-034) |
//!
//! [`decide`] takes the state explicitly and follows `0000-MAIN`, `1000-PROCESS-INPUTS` and
//! `2000-DECIDE-ACTION` for every state, so a state the screen holds but never sends, such as C
//! after a save, still gets the program's answer; the service passes [`served_state`]'s. Every
//! commit re-reads and re-validates: the row is re-read on every endpoint 27 request, and the
//! received code and description are edited again before a write (COTRTUPC-024).
//!
//! # Flow
//!
//! - Endpoint 26 (ENTER in the fresh and X states): [`fetch_key`] edits the code, the service
//!   reads that key, and [`fetch`] answers with state S and the row, with state X and the add
//!   prompt (a 200, not a 404, COTRTUPC-010), or with the fetch error.
//! - Endpoint 27: [`receive_type_code`] gives the key the service re-reads, and [`served_state`]
//!   and [`decide`] give a [`MaintOutcome`]. For [`MaintOutcome::Execute`] the service runs the
//!   [`MaintEffect`] against the repository and passes its [`StoreResult`] to [`apply_result`],
//!   which returns the insert that follows an update that changed no row (COTRTUPC-026), or the
//!   final answer. The service commits after every [`StoreResult::Done`], as the program issues
//!   `EXEC CICS SYNCPOINT`.
//!
//! This module does no I/O and holds no SQL: the repository binds the code and description an
//! effect carries.
//!
//! # Responses
//!
//! A [`MaintFailure`] is an error response: the error row's text, the field it concerns, the
//! status class, and the state the program is left in, from which the screen sets its info row
//! (COTRTUPC-045). 400 is every edit and every key the endpoint does not serve; 500 is every
//! store failure, the lock error included (COTRTUPC-029). 404 and 409 are never returned: a type
//! no row holds starts the add path (COTRTUPC-010), and `Record changed by some one else. Please
//! review` is declared but never set (COTRTUPC-025). The other texts the program declares and
//! never shows (`Selected transaction type shown above`, `No input received`, `Update of record
//! failed`, `Delete of record failed` and the rest of X-COTRTUPC-10) have no constant here.
//! Every text is the program's literal without padding, as the response carries it.
//!
//! # Halts
//!
//! `HALTED(H3)`: each store-failure message is the program's fixed prefix only (COTRTUPC-011,
//! COTRTUPC-027, COTRTUPC-028, COTRTUPC-033, COTRTUPC-034). The edited `SQLCODE`, the colon and
//! `SQLERRM` that the COBOL strings after it, `SQLERRM` twice in the delete-restricted message
//! (E19), are Db2's text with no `PostgreSQL` counterpart (COTRTUPC-049). No part of them is
//! produced, and the store reports only its SQLSTATE ([`StoreResult::from_sqlstate`]).
//!
//! # Input and totality
//!
//! The code and the description enter as the raw text of their map fields, `TRTYPCDI PIC X(2)`
//! and `TRTYDSCI PIC X(50)`, each read as a `MOVE` into its field: its first characters up to the
//! field's width, padded with spaces. An absent description is the empty text. A field is blank
//! when it is all spaces or all low-values (`\0`). Every function takes any text and returns a
//! value or a [`MaintFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input).
//!
//! A code character outside printable ASCII fails as `Tran Type code must be numeric.`. That is
//! the program's result wherever the key edits run, since `FUNCTION TEST-NUMVAL` rejects it, and
//! the replacement's choice, with no parity claim, on the paths that skip them, because the
//! `CHAR(2)` key and [`PicX`] hold printable ASCII only.

use core::fmt;
use std::iter;

use copybook::PicX;

use crate::messages::{ABEND_CODE_9999, ABEND_MSG_UNEXPECTED_DATA, AbendData};
use crate::records::db2_transaction_type::Db2TransactionType;

/// `LIT-THISPGM`, the program name `ABEND-CULPRIT` receives (COTRTUPC-037).
const PROGRAM: &str = "COTRTUPC";

/// Width of `TRTYPCDI PIC X(2)` and `TTUP-NEW-TTYP-TYPE PIC X(02)` (COTRTUPC-004).
const CODE_WIDTH: usize = 2;

/// Width of `TRTYDSCI PIC X(50)` and `TTUP-NEW-TTYP-TYPE-DESC PIC X(50)` (COTRTUPC-004).
const DESCRIPTION_WIDTH: usize = 50;

/// The trailing signs `FUNCTION NUMVAL` accepts after the digits: `+`, `-`, `CR` and `DB`.
const TRAILING_SIGNS: [&[u8]; 4] = [b"+", b"-", b"CR", b"DB"];

// `WS-INFO-MSG PIC X(40)`: the 88-level values `3250-SETUP-INFOMSG` sets (COTRTUPC-045).

/// 88 `PROMPT-FOR-SEARCH-KEYS` (`COTRTUPC.cbl:147-148`).
const PROMPT_FOR_SEARCH_KEYS: &str = "Enter transaction type to be maintained";
/// 88 `PROMPT-CREATE-NEW-RECORD` (`COTRTUPC.cbl:149-150`).
const PROMPT_CREATE_NEW_RECORD: &str = "Press F05 to add. F12 to cancel";
/// 88 `PROMPT-DELETE-CONFIRM` (`COTRTUPC.cbl:151-152`).
const PROMPT_DELETE_CONFIRM: &str = "Delete this record ? Press F4 to confirm";
/// 88 `CONFIRM-DELETE-SUCCESS` (`COTRTUPC.cbl:153-154`).
const CONFIRM_DELETE_SUCCESS: &str = "Delete successful.";
/// 88 `PROMPT-FOR-CHANGES` (`COTRTUPC.cbl:155-156`).
const PROMPT_FOR_CHANGES: &str = "Update transaction type details shown.";
/// 88 `PROMPT-FOR-NEWDATA` (`COTRTUPC.cbl:157-158`).
const PROMPT_FOR_NEWDATA: &str = "Enter new transaction type details.";
/// 88 `PROMPT-FOR-CONFIRMATION` (`COTRTUPC.cbl:160-161`).
const PROMPT_FOR_CONFIRMATION: &str = "Changes validated.Press F5 to save";
/// 88 `CONFIRM-UPDATE-SUCCESS` (`COTRTUPC.cbl:162-163`).
const CONFIRM_UPDATE_SUCCESS: &str = "Changes committed to database";
/// 88 `INFORM-FAILURE` (`COTRTUPC.cbl:164-165`).
const INFORM_FAILURE: &str = "Changes unsuccessful";

// `WS-RETURN-MSG PIC X(75)`: the error-row texts the program shows.

/// 88 `WS-RECORD-NOT-FOUND` (`COTRTUPC.cbl:175-176`; COTRTUPC-010).
const RECORD_NOT_FOUND: &str = "No record found for this key in database";
/// 88 `NO-CHANGES-DETECTED` (`COTRTUPC.cbl:179-180`; COTRTUPC-017).
const NO_CHANGES_DETECTED: &str = "No change detected with respect to values fetched.";
/// 88 `COULD-NOT-LOCK-REC-FOR-UPDATE` (`COTRTUPC.cbl:181-182`; COTRTUPC-029).
const COULD_NOT_LOCK: &str = "Could not lock record for update";
/// 88 `WS-INVALID-KEY-PRESSED` (`COTRTUPC.cbl:193-194`; COTRTUPC-042, COTRTUPC-043).
const INVALID_KEY_PRESSED: &str = "Invalid key pressed";
/// `'Tran Type code'` strung with `' must be supplied.'` (`COTRTUPC.cbl:820-830,913-930`;
/// COTRTUPC-005).
const CODE_SUPPLIED: &str = "Tran Type code must be supplied.";
/// `'Tran Type code'` strung with `' must be numeric.'` (`COTRTUPC.cbl:932-949`; COTRTUPC-006).
const CODE_NUMERIC: &str = "Tran Type code must be numeric.";
/// `'Tran Type code'` strung with `' must not be zero.'` (`COTRTUPC.cbl:952-966`;
/// COTRTUPC-007).
const CODE_NOT_ZERO: &str = "Tran Type code must not be zero.";
/// `'Transaction Desc'` strung with `' must be supplied.'` (`COTRTUPC.cbl:753-764,855-873`;
/// COTRTUPC-018).
const DESCRIPTION_SUPPLIED: &str = "Transaction Desc must be supplied.";
/// `'Transaction Desc'` strung with `' can have numbers or alphabets only.'`
/// (`COTRTUPC.cbl:875-899`; COTRTUPC-019).
const DESCRIPTION_ALPHANUMERIC: &str = "Transaction Desc can have numbers or alphabets only.";

// The fixed prefixes of the store-failure messages. `HALTED(H3)`: the edited SQLCODE, the colon
// and `SQLERRM` the COBOL strings after each prefix are Db2's and are not produced (COTRTUPC-049).

/// `'Error accessing:'` and `' TRANSACTION_TYPE table. SQLCODE:'` (`COTRTUPC.cbl:1499-1507`;
/// COTRTUPC-011).
const FETCH_ERROR: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";
/// `'Error updating:'` and `' TRANSACTION_TYPE Table. SQLCODE:'` (`COTRTUPC.cbl:1569-1577`;
/// COTRTUPC-028).
const UPDATE_ERROR: &str = "Error updating: TRANSACTION_TYPE Table. SQLCODE:";
/// `'Error inserting record into:'` and `' TRANSACTION_TYPE Table. SQLCODE:'`
/// (`COTRTUPC.cbl:1609-1617`; COTRTUPC-027).
const INSERT_ERROR: &str = "Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:";
/// `'Please delete associated child records first:'` and `'SQLCODE :'`
/// (`COTRTUPC.cbl:1640-1649`; COTRTUPC-033, E19).
const DELETE_RESTRICTED: &str = "Please delete associated child records first:SQLCODE :";
/// `'Delete failed with message:'` and `'SQLCODE :'` (`COTRTUPC.cbl:1653-1661`; COTRTUPC-034).
const DELETE_FAILED: &str = "Delete failed with message:SQLCODE :";

/// The key a request carries, `TranTypeActionRequest.aid`: the AID keys COTRTUPC tests, F3
/// apart, which the screen serves alone (COTRTUPC-044).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TtupAid {
    /// ENTER, `CCARD-AID-ENTER`: fetch or validate.
    Enter,
    /// F4, `CCARD-AID-PFK04`: delete.
    Pf4,
    /// F5, `CCARD-AID-PFK05`: save.
    Pf5,
    /// F12, `CCARD-AID-PFK12`: cancel, which the screen performs without a request
    /// (COTRTUPC-039 to COTRTUPC-041).
    Pf12,
}

/// `TTUP-CHANGE-ACTION PIC X(1)` (`COTRTUPC.cbl:296-327`): one variant per value its 88-levels
/// name. The group 88-levels are the methods [`TtupState::is_delete_in_progress`],
/// [`TtupState::is_changes_made`] and [`TtupState::is_changes_failed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TtupState {
    /// Low-values or spaces, 88 `TTUP-DETAILS-NOT-FETCHED`: the fresh screen, no type fetched.
    DetailsNotFetched,
    /// `K`, 88 `TTUP-INVALID-SEARCH-KEYS`: set and replaced by the next statement
    /// (`COTRTUPC.cbl:729-730`), so it never outlives a step.
    InvalidSearchKeys,
    /// `X`, 88 `TTUP-DETAILS-NOT-FOUND`: no row holds the code (COTRTUPC-010).
    DetailsNotFound,
    /// `S`, 88 `TTUP-SHOW-DETAILS`: a fetched row shown (COTRTUPC-009).
    ShowDetails,
    /// `R`, 88 `TTUP-CREATE-NEW-RECORD`: a new type being entered (COTRTUPC-015).
    CreateNewRecord,
    /// `V`, 88 `TTUP-REVIEW-NEW-RECORD`: declared and never set.
    ReviewNewRecord,
    /// `9`, 88 `TTUP-CONFIRM-DELETE`: a delete awaiting its confirming F4 (COTRTUPC-031).
    ConfirmDelete,
    /// `8`, 88 `TTUP-START-DELETE`: the state a delete refused by categories leaves
    /// (COTRTUPC-033, COTRTUPC-035).
    StartDelete,
    /// `7`, 88 `TTUP-DELETE-DONE` (COTRTUPC-032).
    DeleteDone,
    /// `6`, 88 `TTUP-DELETE-FAILED` (COTRTUPC-034).
    DeleteFailed,
    /// `E`, 88 `TTUP-CHANGES-NOT-OK`: entered values that failed an edit (COTRTUPC-018).
    ChangesNotOk,
    /// `N`, 88 `TTUP-CHANGES-OK-NOT-CONFIRMED`: validated values awaiting F5 (COTRTUPC-020).
    ChangesOkNotConfirmed,
    /// `L`, 88 `TTUP-CHANGES-OKAYED-LOCK-ERROR` (COTRTUPC-029).
    ChangesOkayedLockError,
    /// `F`, 88 `TTUP-CHANGES-OKAYED-BUT-FAILED`: unreachable, the failed `STRING` overwriting
    /// the text that would set it (COTRTUPC-028).
    ChangesOkayedButFailed,
    /// `C`, 88 `TTUP-CHANGES-OKAYED-AND-DONE`: the save ended, successfully or not
    /// (COTRTUPC-024, COTRTUPC-027, COTRTUPC-028).
    ChangesOkayedAndDone,
    /// `B`, 88 `TTUP-CHANGES-BACKED-OUT`: F12 in state N (COTRTUPC-039).
    ChangesBackedOut,
}

impl TtupState {
    /// 88 `TTUP-DELETE-IN-PROGRESS`: `9`, `8`, `7` or `6`.
    #[must_use]
    pub const fn is_delete_in_progress(self) -> bool {
        matches!(
            self,
            Self::ConfirmDelete | Self::StartDelete | Self::DeleteDone | Self::DeleteFailed
        )
    }

    /// 88 `TTUP-CHANGES-MADE`: `E`, `N`, `L` or `F`.
    #[must_use]
    pub const fn is_changes_made(self) -> bool {
        matches!(
            self,
            Self::ChangesNotOk
                | Self::ChangesOkNotConfirmed
                | Self::ChangesOkayedLockError
                | Self::ChangesOkayedButFailed
        )
    }

    /// 88 `TTUP-CHANGES-FAILED`: `L` or `F`.
    #[must_use]
    pub const fn is_changes_failed(self) -> bool {
        matches!(
            self,
            Self::ChangesOkayedLockError | Self::ChangesOkayedButFailed
        )
    }

    /// The info row `3250-SETUP-INFOMSG` sets for the state (COTRTUPC-045), without the centring
    /// the screen applies.
    ///
    /// State B shows `Update transaction type details shown.` when it holds fetched values, the
    /// only B a server step leaves; B with blank old values shows
    /// `Enter transaction type to be maintained`, which the screen sets itself (COTRTUPC-039).
    /// `Selected transaction type shown above` is never shown: `TTUP-SHOW-DETAILS` shares the
    /// search prompt (X-COTRTUPC-3).
    #[must_use]
    pub const fn info_message(self) -> &'static str {
        match self {
            Self::DetailsNotFetched
            | Self::InvalidSearchKeys
            | Self::ShowDetails
            | Self::ReviewNewRecord
            | Self::StartDelete => PROMPT_FOR_SEARCH_KEYS,
            Self::DetailsNotFound => PROMPT_CREATE_NEW_RECORD,
            Self::CreateNewRecord => PROMPT_FOR_NEWDATA,
            Self::ChangesNotOk | Self::ChangesBackedOut => PROMPT_FOR_CHANGES,
            Self::ChangesOkNotConfirmed => PROMPT_FOR_CONFIRMATION,
            Self::ChangesOkayedAndDone => CONFIRM_UPDATE_SUCCESS,
            Self::ChangesOkayedLockError | Self::ChangesOkayedButFailed | Self::DeleteFailed => {
                INFORM_FAILURE
            }
            Self::ConfirmDelete => PROMPT_DELETE_CONFIRM,
            Self::DeleteDone => CONFIRM_DELETE_SUCCESS,
        }
    }
}

/// The request field a failure concerns: where the program puts the cursor and the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaintField {
    /// `TRTYPCD`, the `{type_code}` path parameter.
    TypeCode,
    /// `TRTYDSC`, `TranTypeActionRequest.description`.
    Description,
}

/// The status class of a [`MaintFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit failed, or the endpoint does not serve the key: 400.
    Input,
    /// The store failed: 500 (COTRTUPC-011, COTRTUPC-027 to COTRTUPC-029, COTRTUPC-033,
    /// COTRTUPC-034, COTRTUPC-038).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 500 for
    /// [`FailureClass::Store`]. No COTRTUPC outcome is a 404 or a 409.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::Store => 500,
        }
    }
}

/// A screen COTRTUPC sends with an error row instead of the step's result: the message, the field
/// it concerns, the status class and the state the program is left in.
///
/// The message is the literal the program moves or strings into `WS-RETURN-MSG PIC X(75)`,
/// without padding; a store failure's is its fixed prefix only, `HALTED(H3)` (COTRTUPC-049).
/// The message of a failure no row names is blank, as the COBOL's `ABEND-MSG` is
/// (COTRTUPC-038). No message holds a request value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MaintFailure {
    message: &'static str,
    field: Option<MaintField>,
    class: FailureClass,
    state: TtupState,
}

impl MaintFailure {
    /// An edit failure or an unserved key: status class [`FailureClass::Input`].
    const fn input(message: &'static str, field: Option<MaintField>, state: TtupState) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Input,
            state,
        }
    }

    /// A store failure: status class [`FailureClass::Store`].
    const fn store(message: &'static str, field: Option<MaintField>, state: TtupState) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Store,
            state,
        }
    }

    /// A failure no row of the audit names: 500 with the blank `ABEND-MSG`, nothing written
    /// (COTRTUPC-038).
    const fn unexpected(state: TtupState) -> Self {
        Self::store("", None, state)
    }

    /// The error row, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the message concerns, if any.
    #[must_use]
    pub const fn field(&self) -> Option<MaintField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }

    /// The state the program is left in, which names the screen's info row (COTRTUPC-045).
    #[must_use]
    pub const fn state(&self) -> TtupState {
        self.state
    }
}

impl fmt::Display for MaintFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for MaintFailure {}

/// What the read of a code found: `SELECT TR_TYPE, TR_DESCRIPTION … WHERE TR_TYPE = :code`
/// (`9100-GET-TRANSACTION-TYPE`, `COTRTUPC.cbl:1469-1488`).
///
/// For endpoint 26 it is the program's read (COTRTUPC-009 to COTRTUPC-011); for endpoint 27 it
/// is the service's re-read that stands in for `TTUP-OLD-DETAILS`, the fetched values the
/// COMMAREA kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeLookup<'a> {
    /// SQLCODE 0: the row.
    Found(&'a Db2TransactionType),
    /// SQLCODE +100: no row holds the code.
    NotFound,
    /// A negative SQLCODE: the read failed.
    Error,
}

/// The two inputs of `CTRTUPA`, each the raw text of its map field. An absent field is the
/// empty text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaintInput<'a> {
    /// `TRTYPCDI PIC X(2)`: the `{type_code}` path parameter, the code the screen shows.
    pub type_code: &'a str,
    /// `TRTYDSCI PIC X(50)`: `TranTypeActionRequest.description`.
    pub description: &'a str,
}

/// A screen COTRTUPC sends without an edit or store failure: the state it leaves, the code and
/// description it shows, its error row and its info row.
///
/// The error row is empty on every such screen but the one for a code no row holds
/// (`No record found for this key in database`, COTRTUPC-010).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeView {
    state: TtupState,
    code: PicX<2>,
    description: Option<String>,
    message: Option<&'static str>,
}

impl TranTypeView {
    /// The screen of `state` showing `code` and `description`, with error row `message`.
    const fn new(
        state: TtupState,
        code: PicX<2>,
        description: Option<String>,
        message: Option<&'static str>,
    ) -> Self {
        Self {
            state,
            code,
            description,
            message,
        }
    }

    /// The fresh screen of `0000-MAIN`'s initial entry: code and description empty, error row
    /// empty, the search prompt (COTRTUPC-003, COTRTUPC-030).
    const fn fresh() -> Self {
        Self::new(TtupState::DetailsNotFetched, PicX::spaces(), None, None)
    }

    /// The state the screen enters.
    #[must_use]
    pub const fn state(&self) -> TtupState {
        self.state
    }

    /// The code shown, `TRTYPCDO PIC X(2)`: the stored code of a fetched row, the normalised key
    /// of a code no row holds, or the code validated or saved. Spaces on the fresh screen.
    #[must_use]
    pub const fn code(&self) -> &PicX<2> {
        &self.code
    }

    /// The description shown, `TRTYDSCO PIC X(50)`: a fetched row's stored text up to its
    /// stored length (COTRTUPC-009), or the 50-character value validated or saved
    /// (COTRTUPC-020, COTRTUPC-023). `None` when the step shows none, or after a delete, whose
    /// screen keeps showing the fetched values (COTRTUPC-030).
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// The error row, `WS-RETURN-MSG`, when the screen has one.
    #[must_use]
    pub const fn message(&self) -> Option<&'static str> {
        self.message
    }

    /// The info row, `WS-INFO-MSG`, for the state the screen enters (COTRTUPC-045).
    #[must_use]
    pub const fn info_message(&self) -> &'static str {
        self.state.info_message()
    }
}

/// A write the service runs against `db2.transaction_type` for a step, then reports to
/// [`apply_result`] (Thin Request Handlers).
///
/// The code and description are the host variables `DCL-TR-TYPE` and `DCL-TR-DESCRIPTION`
/// (`DCLTRTYP.dcl:38-46`). The description is the `FUNCTION TRIM` of the entered text,
/// space-padded to the 50 bytes `FUNCTION LENGTH(TTUP-NEW-TTYP-TYPE-DESC)` gives its length
/// (`COTRTUPC.cbl:1538-1542`; COTRTUPC-023): letters, digits and spaces only, so
/// [`Db2TransactionType::new`] accepts the pair and `TR_DESCRIPTION VARCHAR(50)` holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintEffect {
    /// `UPDATE … SET TR_DESCRIPTION = :desc WHERE TR_TYPE = :code` (`9600-WRITE-PROCESSING`;
    /// COTRTUPC-024, COTRTUPC-025: nothing is compared with the fetched values).
    Update {
        /// `DCL-TR-TYPE`.
        code: PicX<2>,
        /// `DCL-TR-DESCRIPTION`, 50 bytes.
        description: String,
    },
    /// `INSERT … (TR_TYPE, TR_DESCRIPTION) VALUES (:code, :desc)` after an update that changed
    /// no row (`9700-INSERT-RECORD`; COTRTUPC-026).
    Insert {
        /// `DCL-TR-TYPE`.
        code: PicX<2>,
        /// `DCL-TR-DESCRIPTION`, 50 bytes.
        description: String,
    },
    /// `DELETE … WHERE TR_TYPE = :code`, the fetched code (`9800-DELETE-PROCESSING`;
    /// COTRTUPC-032). `db2.transaction_type_category` refuses it while a category refers to
    /// the code (`ON DELETE RESTRICT`, `TRNTYCAT.ddl:5-7`; COTRTUPC-033).
    Delete {
        /// `DCL-TR-TYPE`.
        code: PicX<2>,
    },
}

/// How the store ended a [`MaintEffect`]: the `PostgreSQL` outcome that stands for each Db2
/// SQLCODE the program tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreResult {
    /// SQLCODE 0: the statement changed its row. The service commits.
    Done,
    /// SQLCODE +100: the `UPDATE` or `DELETE` changed no row.
    NoRow,
    /// SQLCODE -911: `deadlock_detected` (`40P01`) or `lock_not_available` (`55P03`).
    LockNotAvailable,
    /// SQLCODE -532: `foreign_key_violation` (`23503`), a category row refers to the code.
    ForeignKeyViolation,
    /// Any other negative SQLCODE: every other SQLSTATE.
    OtherError,
}

impl StoreResult {
    /// The result a failed statement's SQLSTATE stands for, which is all the store reports
    /// (`persistence::error`): `23503` is -532, `40P01` and `55P03` are -911, and every other
    /// state is a negative SQLCODE the program does not name (COTRTUPC-029, COTRTUPC-033).
    #[must_use]
    pub fn from_sqlstate(sqlstate: &str) -> Self {
        match sqlstate {
            "23503" => Self::ForeignKeyViolation,
            "40P01" | "55P03" => Self::LockNotAvailable,
            _ => Self::OtherError,
        }
    }
}

/// What a step of endpoint 27 yields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintOutcome {
    /// A 200 with the screen the step sends; nothing (more) is written.
    Screen(TranTypeView),
    /// A write the service runs, passing its result to [`apply_result`].
    Execute(MaintEffect),
    /// An error response; nothing (more) is written.
    Failed(MaintFailure),
    /// `ABEND-ROUTINE` after the `WHEN OTHER` of `2000-DECIDE-ACTION`: the abend data replaces
    /// the screen, the conversation ends and nothing is written (COTRTUPC-037). Code `9999`,
    /// which replaces the `0001` the branch moves, culprit `COTRTUPC`, reason blank, message
    /// `UNEXPECTED DATA SCENARIO`. Only states the screen never sends reach it (COTRTUPC-035,
    /// COTRTUPC-036).
    Abend(AbendData),
}

/// `0001-CHECK-PFKEYS` (`COTRTUPC.cbl:577-608`): whether `state` accepts `aid` (COTRTUPC-042).
///
/// ENTER is accepted except in state 9; F4 in S and 9; F5 in N, X, 9, 8, 7 and 6; F12 in N,
/// S, X, 9 and R. F3, always accepted, is the screen's (COTRTUPC-044). A key the state rejects is
/// not processed as ENTER: the program sends the current screen again with the error row
/// `Invalid key pressed` and receives nothing typed. The error row is always free when the check
/// runs, `0000-MAIN` having just cleared it.
///
/// # Errors
///
/// Status class [`FailureClass::Input`], message `Invalid key pressed`, no field and `state`
/// unchanged, when `state` rejects `aid` (COTRTUPC-042, COTRTUPC-043).
pub fn check_pfkeys(aid: TtupAid, state: TtupState) -> Result<TtupAid, MaintFailure> {
    let accepted = match aid {
        TtupAid::Enter => !matches!(state, TtupState::ConfirmDelete),
        TtupAid::Pf4 => matches!(state, TtupState::ShowDetails | TtupState::ConfirmDelete),
        TtupAid::Pf5 => {
            matches!(
                state,
                TtupState::ChangesOkNotConfirmed | TtupState::DetailsNotFound
            ) || state.is_delete_in_progress()
        }
        TtupAid::Pf12 => matches!(
            state,
            TtupState::ChangesOkNotConfirmed
                | TtupState::ShowDetails
                | TtupState::DetailsNotFound
                | TtupState::ConfirmDelete
                | TtupState::CreateNewRecord
        ),
    };
    if accepted {
        Ok(aid)
    } else {
        Err(invalid_key(state))
    }
}

/// `1150-STORE-MAP-IN-NEW` for the code, with the blank test of `1245-EDIT-NUM-REQD`: the key
/// endpoint 27 re-reads and the code it saves or deletes (COTRTUPC-004, COTRTUPC-005,
/// COTRTUPC-024).
///
/// The raw text is fitted to `TRTYPCDI PIC X(2)`; `*` alone, spaces or low-values are blank, and
/// any other value loses its leading and trailing spaces and is stored left-justified and
/// space-padded, so ` 9` and `9` both give `9 `. Nothing else is edited or normalised.
///
/// # Errors
///
/// Status class [`FailureClass::Input`], field [`MaintField::TypeCode`], the fresh state:
///
/// - a blank code: `Tran Type code must be supplied.` (COTRTUPC-005);
/// - a code holding a character outside printable ASCII: `Tran Type code must be numeric.`,
///   the replacement's choice on the paths that skip the key edits (module documentation).
pub fn receive_type_code(raw: &str) -> Result<PicX<2>, MaintFailure> {
    match store_code(raw) {
        StoredCode::Code(code) => Ok(code),
        StoredCode::Blank => Err(code_failure(CODE_SUPPLIED)),
        StoredCode::Unreadable => Err(code_failure(CODE_NUMERIC)),
    }
}

/// `1150-STORE-MAP-IN-NEW` and `1210-EDIT-TRANTYPE` with `1245-EDIT-NUM-REQD`: the edited and
/// normalised type code (COTRTUPC-004 to COTRTUPC-008).
///
/// A stored code [`receive_type_code`] accepts must pass `FUNCTION TEST-NUMVAL`: optional
/// leading and trailing spaces, at most one sign (a leading or trailing `+` or `-`, or a
/// trailing `CR` or `DB`) and digits with at most one decimal point. Its `FUNCTION NUMVAL` must
/// not be zero. The code is then the two digits of the absolute value of its integer part, as
/// `COMPUTE` stores `NUMVAL` into the unsigned `WS-EDIT-NUMERIC-2 PIC 9(02)` without `ROUNDED`:
/// `1`, `01`, `+1`, `-1`, `1-` and `1.` all give `01`, and `.5` passes the zero test and gives
/// `00` (X-COTRTUPC-13).
///
/// # Errors
///
/// Status class [`FailureClass::Input`], field [`MaintField::TypeCode`], the fresh state, with
/// the first message that applies:
///
/// - `Tran Type code must be supplied.` for a blank code (COTRTUPC-005);
/// - `Tran Type code must be numeric.` for a code `TEST-NUMVAL` rejects, such as `AB`, `1A`,
///   `-`, `.` or `CR` (COTRTUPC-006);
/// - `Tran Type code must not be zero.` for a code whose `NUMVAL` is 0, such as `00`, `0`,
///   `-0`, `0.` or `.0` (COTRTUPC-007).
pub fn edit_type_code(raw: &str) -> Result<PicX<2>, MaintFailure> {
    let code = receive_type_code(raw)?;
    match numval(code.as_bytes()) {
        None => Err(code_failure(CODE_NUMERIC)),
        Some(value) if value.zero => Err(code_failure(CODE_NOT_ZERO)),
        Some(value) => Ok(PicX::from_array(value.units)),
    }
}

/// The key endpoint 26 reads for ENTER in state `prior`: [`edit_type_code`]'s, except in state
/// X (COTRTUPC-005 to COTRTUPC-008, COTRTUPC-012).
///
/// In state X a non-blank code skips the key edits and the normalisation: `1200-EDIT-MAP-INPUTS`
/// selects it as `1150-STORE-MAP-IN-NEW` stored it, so a code such as `AB`, `00` or `9 ` reaches
/// the add path (X-COTRTUPC-1). How the screen tells endpoint 26 that it is in state X is the
/// API's; a code read without the marker gets the key edits.
///
/// # Errors
///
/// As [`edit_type_code`], or in state X as [`receive_type_code`].
pub fn fetch_key(raw: &str, prior: TtupState) -> Result<PicX<2>, MaintFailure> {
    match (prior, store_code(raw)) {
        (TtupState::DetailsNotFound, StoredCode::Code(code)) => Ok(code),
        _ => edit_type_code(raw),
    }
}

/// `1150-STORE-MAP-IN-NEW` and `1230-EDIT-ALPHANUM-REQD` for the description: the value
/// `9600-WRITE-PROCESSING` binds (COTRTUPC-004, COTRTUPC-018, COTRTUPC-019, COTRTUPC-023).
///
/// The raw text is fitted to `TRTYDSCI PIC X(50)`; `*` alone, spaces or low-values are blank,
/// and any other value loses its leading and trailing spaces. The result is that text
/// space-padded to 50 bytes: the `FUNCTION TRIM` of the stored field, at the length
/// `FUNCTION LENGTH` of the 50-byte field gives it, case as entered (X-COTRTUPC-7,
/// X-COTRTUPC-14). It holds letters, digits and spaces only, so it satisfies the `VARCHAR(50)`
/// rule of [`Db2TransactionType::new`].
///
/// # Errors
///
/// Status class [`FailureClass::Input`], field [`MaintField::Description`], state E:
///
/// - `Transaction Desc must be supplied.` for a blank description (COTRTUPC-018);
/// - `Transaction Desc can have numbers or alphabets only.` for one holding any character but
///   `A` to `Z`, `a` to `z`, `0` to `9` and the space (COTRTUPC-019).
pub fn edit_description(raw: &str) -> Result<String, MaintFailure> {
    let field = store_description(raw);
    edit_alphanum_reqd(&field).map(|()| field)
}

/// `2000-DECIDE-ACTION` after `9000-READ-TRANTYPE` for `key`: endpoint 26's answer
/// (COTRTUPC-009 to COTRTUPC-011).
///
/// A row gives state S with the stored code and the stored description, the text up to its
/// stored length, trailing spaces kept; the info row is the search prompt (COTRTUPC-009,
/// X-COTRTUPC-3). No row gives state X with `key`, no description, error row `No record found
/// for this key in database` and info row `Press F05 to add. F12 to cancel`: a 200, since the
/// add path continues from it (COTRTUPC-010).
///
/// # Errors
///
/// A failed read: status class [`FailureClass::Store`], field [`MaintField::TypeCode`], message
/// `Error accessing: TRANSACTION_TYPE table. SQLCODE:`, the rest `HALTED(H3)` (COTRTUPC-049).
/// The program then takes the not-found path, so the state is X (COTRTUPC-011).
pub fn fetch(key: &PicX<2>, lookup: TypeLookup<'_>) -> Result<TranTypeView, MaintFailure> {
    match lookup {
        TypeLookup::Found(row) => Ok(TranTypeView::new(
            TtupState::ShowDetails,
            row.tr_type().clone(),
            Some(row.tr_description().to_owned()),
            None,
        )),
        TypeLookup::NotFound => Ok(TranTypeView::new(
            TtupState::DetailsNotFound,
            key.clone(),
            None,
            Some(RECORD_NOT_FOUND),
        )),
        TypeLookup::Error => Err(MaintFailure::store(
            FETCH_ERROR,
            Some(MaintField::TypeCode),
            TtupState::DetailsNotFound,
        )),
    }
}

/// The state endpoint 27 serves `aid` in, from the row the service re-read for `{type_code}`,
/// the audit's endpoint mapping (module documentation; X-COTRTUPC-2).
///
/// ENTER is state S when a row holds the code and state R when none does (COTRTUPC-016,
/// COTRTUPC-021); PF5 is state N (COTRTUPC-024); PF4 is state 9 (COTRTUPC-032). F12 is never
/// served, and [`decide`] answers it before reading the state (COTRTUPC-043); it maps as ENTER.
#[must_use]
pub const fn served_state(aid: TtupAid, current: TypeLookup<'_>) -> TtupState {
    match aid {
        TtupAid::Pf4 => TtupState::ConfirmDelete,
        TtupAid::Pf5 => TtupState::ChangesOkNotConfirmed,
        TtupAid::Enter | TtupAid::Pf12 => match current {
            TypeLookup::NotFound => TtupState::CreateNewRecord,
            TypeLookup::Found(_) | TypeLookup::Error => TtupState::ShowDetails,
        },
    }
}

/// `0000-MAIN` with `1000-PROCESS-INPUTS` and `2000-DECIDE-ACTION` for a request of endpoint 27:
/// `aid` in `state`, with the raw `input` and `current`, the service's re-read of the code that
/// stands in for the fetched values (COTRTUPC-016 to COTRTUPC-024, COTRTUPC-032, COTRTUPC-038,
/// COTRTUPC-043).
///
/// The service passes [`served_state`]'s state. The steps, in the program's order:
///
/// 1. F12 is not served: 400 `Invalid key pressed`, nothing read or written (COTRTUPC-043).
/// 2. States C, L, F, 7 and 6, and B with no row, restart as the fresh screen on any key
///    (COTRTUPC-030, COTRTUPC-039).
/// 3. [`check_pfkeys`]: a key the state rejects is 400 `Invalid key pressed` (COTRTUPC-042).
/// 4. ENTER in the fresh, K and X states is the fetch: [`fetch_key`] and [`fetch`], `current`
///    being the read of the key [`fetch_key`] gives.
/// 5. Any other step with a failed re-read is 500 with no COBOL text (COTRTUPC-038).
/// 6. PF4 in state 9 deletes the shown code ([`MaintEffect::Delete`], COTRTUPC-032); PF4 in
///    state S asks for the confirmation, state 9 with the fetched values (COTRTUPC-031).
/// 7. PF5 in state X starts the add, state R (COTRTUPC-015); PF5 in state N saves: the
///    received code and description are edited again ([`receive_type_code`],
///    [`edit_description`]) and the update is returned ([`MaintEffect::Update`],
///    COTRTUPC-023, COTRTUPC-024).
/// 8. ENTER in state N runs no edit and shows the same screen, with the no-change message when
///    the values equal the fetched ones (COTRTUPC-022).
/// 9. Every other step edits the inputs (`1200-EDIT-MAP-INPUTS`). The key edits run when a row
///    holds the code, outside state R, and the entered values are compared with that row;
///    otherwise the received code is kept and the comparison is with blanks, which always finds
///    a change (COTRTUPC-016, COTRTUPC-021). No change is 400 `No change detected with respect to
///    values fetched.`, S staying S and B becoming E (COTRTUPC-017); a change runs the
///    description edits, whose failure leaves state E (COTRTUPC-018, COTRTUPC-019) and whose
///    success is state N, `Changes validated.Press F5 to save`, with nothing written
///    (COTRTUPC-020). No change in states 9, 8 and V ends in the abend of COTRTUPC-037, as no
///    `WHEN` of `2000-DECIDE-ACTION` serves them (COTRTUPC-035, COTRTUPC-036).
#[must_use]
pub fn decide(
    aid: TtupAid,
    state: TtupState,
    input: &MaintInput<'_>,
    current: TypeLookup<'_>,
) -> MaintOutcome {
    if aid == TtupAid::Pf12 {
        return MaintOutcome::Failed(invalid_key(state));
    }
    if let Some(outcome) = initial_entry(state, current) {
        return outcome;
    }
    if let Err(failure) = check_pfkeys(aid, state) {
        return MaintOutcome::Failed(failure);
    }
    match (aid, state) {
        (
            TtupAid::Enter,
            TtupState::DetailsNotFetched
            | TtupState::InvalidSearchKeys
            | TtupState::DetailsNotFound,
        ) => fetch_step(input, state, current),
        _ if matches!(current, TypeLookup::Error) => {
            MaintOutcome::Failed(MaintFailure::unexpected(state))
        }
        (TtupAid::Pf4, TtupState::ConfirmDelete) => delete_step(input),
        // `check_pfkeys` leaves F4 only states S and 9.
        (TtupAid::Pf4, _) => confirm_delete_step(current),
        (TtupAid::Pf5, TtupState::DetailsNotFound) => create_step(input),
        (TtupAid::Pf5, TtupState::ChangesOkNotConfirmed) => save_step(input),
        (TtupAid::Enter, TtupState::ChangesOkNotConfirmed) => review_step(input, current),
        _ => edit_step(state, input, current),
    }
}

/// The answer to `effect` once the store ended it as `result`: the screen after the write, the
/// insert that follows an update that changed no row, or the store failure (COTRTUPC-024 to
/// COTRTUPC-029, COTRTUPC-032 to COTRTUPC-034).
///
/// - [`MaintEffect::Update`]: [`StoreResult::Done`] is state C, info row `Changes committed to
///   database`, showing the values saved (COTRTUPC-024); [`StoreResult::NoRow`] returns the
///   [`MaintEffect::Insert`] of the same values (COTRTUPC-026); a lock error is 500 `Could not
///   lock record for update`, state L (COTRTUPC-029); any other failure is 500 `Error updating:
///   TRANSACTION_TYPE Table. SQLCODE:` with state C, whose info row says the changes were
///   committed although nothing was written (COTRTUPC-028, X-COTRTUPC-4).
/// - [`MaintEffect::Insert`]: [`StoreResult::Done`] is state C as for the update
///   (COTRTUPC-026); any other result is 500 `Error inserting record into: TRANSACTION_TYPE
///   Table. SQLCODE:` with state C (COTRTUPC-027).
/// - [`MaintEffect::Delete`]: [`StoreResult::Done`] is state 7, `Delete successful.`
///   (COTRTUPC-032); a category still referring to the code is 500 `Please delete associated
///   child records first:SQLCODE :` with state 8 and nothing deleted (COTRTUPC-033, E19); any
///   other result, no row deleted included, is 500 `Delete failed with message:SQLCODE :` with
///   state 6 and the code in error (COTRTUPC-034).
///
/// `HALTED(H3)`: every store-failure message stops at the program's fixed prefix; the SQLCODE and
/// `SQLERRM` the COBOL strings after it are Db2's (COTRTUPC-049).
#[must_use]
pub fn apply_result(effect: MaintEffect, result: StoreResult) -> MaintOutcome {
    match effect {
        MaintEffect::Update { code, description } => updated(code, description, result),
        MaintEffect::Insert { code, description } => inserted(code, description, result),
        MaintEffect::Delete { code } => deleted(code, result),
    }
}

/// The result of `9600-WRITE-PROCESSING`'s `UPDATE` (COTRTUPC-024, COTRTUPC-026,
/// COTRTUPC-028, COTRTUPC-029).
fn updated(code: PicX<2>, description: String, result: StoreResult) -> MaintOutcome {
    match result {
        StoreResult::Done => committed(code, description),
        StoreResult::NoRow => MaintOutcome::Execute(MaintEffect::Insert { code, description }),
        StoreResult::LockNotAvailable => MaintOutcome::Failed(MaintFailure::store(
            COULD_NOT_LOCK,
            None,
            TtupState::ChangesOkayedLockError,
        )),
        StoreResult::ForeignKeyViolation | StoreResult::OtherError => MaintOutcome::Failed(
            MaintFailure::store(UPDATE_ERROR, None, TtupState::ChangesOkayedAndDone),
        ),
    }
}

/// The result of `9700-INSERT-RECORD`, whose `WHEN OTHER` takes every SQLCODE but 0
/// (COTRTUPC-026, COTRTUPC-027).
fn inserted(code: PicX<2>, description: String, result: StoreResult) -> MaintOutcome {
    match result {
        StoreResult::Done => committed(code, description),
        StoreResult::NoRow
        | StoreResult::LockNotAvailable
        | StoreResult::ForeignKeyViolation
        | StoreResult::OtherError => MaintOutcome::Failed(MaintFailure::store(
            INSERT_ERROR,
            None,
            TtupState::ChangesOkayedAndDone,
        )),
    }
}

/// The result of `9800-DELETE-PROCESSING` (COTRTUPC-032 to COTRTUPC-034).
fn deleted(code: PicX<2>, result: StoreResult) -> MaintOutcome {
    match result {
        StoreResult::Done => {
            MaintOutcome::Screen(TranTypeView::new(TtupState::DeleteDone, code, None, None))
        }
        // `-532` sets only `RECORD-DELETE-FAILED`, so the state stays 8 (X-COTRTUPC-6).
        StoreResult::ForeignKeyViolation => MaintOutcome::Failed(MaintFailure::store(
            DELETE_RESTRICTED,
            None,
            TtupState::StartDelete,
        )),
        StoreResult::NoRow | StoreResult::LockNotAvailable | StoreResult::OtherError => {
            MaintOutcome::Failed(MaintFailure::store(
                DELETE_FAILED,
                Some(MaintField::TypeCode),
                TtupState::DeleteFailed,
            ))
        }
    }
}

/// State C after a committed update or insert, showing the values saved (COTRTUPC-024,
/// COTRTUPC-026).
fn committed(code: PicX<2>, description: String) -> MaintOutcome {
    MaintOutcome::Screen(TranTypeView::new(
        TtupState::ChangesOkayedAndDone,
        code,
        Some(description),
        None,
    ))
}

/// The "simulate initial entry" `EVALUATE` of `0000-MAIN` (`COTRTUPC.cbl:403-419`): the states
/// that restart as the fresh screen whatever the key, before the key check (COTRTUPC-030,
/// COTRTUPC-039, COTRTUPC-042). B restarts only with blank old values, a re-read that found no
/// row; B whose re-read failed cannot be decided (COTRTUPC-038).
fn initial_entry(state: TtupState, current: TypeLookup<'_>) -> Option<MaintOutcome> {
    match (state, current) {
        (
            TtupState::ChangesOkayedAndDone
            | TtupState::ChangesOkayedLockError
            | TtupState::ChangesOkayedButFailed
            | TtupState::DeleteDone
            | TtupState::DeleteFailed,
            _,
        )
        | (TtupState::ChangesBackedOut, TypeLookup::NotFound) => {
            Some(MaintOutcome::Screen(TranTypeView::fresh()))
        }
        (TtupState::ChangesBackedOut, TypeLookup::Error) => {
            Some(MaintOutcome::Failed(MaintFailure::unexpected(state)))
        }
        _ => None,
    }
}

/// ENTER in the fresh, K and X states: the fetch of endpoint 26 (COTRTUPC-005 to COTRTUPC-012).
fn fetch_step(input: &MaintInput<'_>, state: TtupState, current: TypeLookup<'_>) -> MaintOutcome {
    match fetch_key(input.type_code, state).and_then(|key| fetch(&key, current)) {
        Ok(view) => MaintOutcome::Screen(view),
        Err(failure) => MaintOutcome::Failed(failure),
    }
}

/// F4 in state 9: the delete of the code shown, which is the fetched code (COTRTUPC-032).
/// No input is received.
fn delete_step(input: &MaintInput<'_>) -> MaintOutcome {
    match receive_type_code(input.type_code) {
        Ok(code) => MaintOutcome::Execute(MaintEffect::Delete { code }),
        Err(failure) => MaintOutcome::Failed(failure),
    }
}

/// F4 in state S: state 9 with the fetched values and the confirmation prompt; nothing typed is
/// received (COTRTUPC-031).
fn confirm_delete_step(current: TypeLookup<'_>) -> MaintOutcome {
    let (code, description) = match current {
        TypeLookup::Found(row) => (row.tr_type().clone(), Some(row.tr_description().to_owned())),
        TypeLookup::NotFound | TypeLookup::Error => (PicX::spaces(), None),
    };
    MaintOutcome::Screen(TranTypeView::new(
        TtupState::ConfirmDelete,
        code,
        description,
        None,
    ))
}

/// F5 in state X: state R with the not-found key and an empty description (COTRTUPC-015).
fn create_step(input: &MaintInput<'_>) -> MaintOutcome {
    match receive_type_code(input.type_code) {
        Ok(code) => MaintOutcome::Screen(TranTypeView::new(
            TtupState::CreateNewRecord,
            code,
            None,
            None,
        )),
        Err(failure) => MaintOutcome::Failed(failure),
    }
}

/// F5 in state N: the update of the received values, edited again (COTRTUPC-023, COTRTUPC-024).
fn save_step(input: &MaintInput<'_>) -> MaintOutcome {
    let code = match receive_type_code(input.type_code) {
        Ok(code) => code,
        Err(failure) => return MaintOutcome::Failed(failure),
    };
    match edit_description(input.description) {
        Ok(description) => MaintOutcome::Execute(MaintEffect::Update { code, description }),
        Err(failure) => MaintOutcome::Failed(failure),
    }
}

/// ENTER in state N: no edit runs and the validated screen is shown again; the comparison still
/// sets its message when the values equal the fetched ones (COTRTUPC-022).
fn review_step(input: &MaintInput<'_>, current: TypeLookup<'_>) -> MaintOutcome {
    let code = match receive_type_code(input.type_code) {
        Ok(code) => code,
        Err(failure) => return MaintOutcome::Failed(failure),
    };
    let description = store_description(input.description);
    let old = old_values(TtupState::ChangesOkNotConfirmed, current);
    if unchanged(&code, &description, old) {
        MaintOutcome::Failed(no_change(TtupState::ChangesOkNotConfirmed))
    } else {
        validated(code, description)
    }
}

/// `1200-EDIT-MAP-INPUTS` and `2000-DECIDE-ACTION` for ENTER in S, E, B, R, V and 8, and F5 in
/// 9 and 8 (COTRTUPC-016 to COTRTUPC-021, COTRTUPC-035 to COTRTUPC-037).
fn edit_step(state: TtupState, input: &MaintInput<'_>, current: TypeLookup<'_>) -> MaintOutcome {
    let old = old_values(state, current);
    let received = if old.is_some() {
        edit_type_code(input.type_code)
    } else {
        receive_type_code(input.type_code)
    };
    let code = match received {
        Ok(code) => code,
        Err(failure) => return MaintOutcome::Failed(failure),
    };
    let description = store_description(input.description);
    if unchanged(&code, &description, old) {
        return match state {
            TtupState::ShowDetails | TtupState::ChangesNotOk => {
                MaintOutcome::Failed(no_change(state))
            }
            TtupState::ChangesBackedOut => MaintOutcome::Failed(no_change(TtupState::ChangesNotOk)),
            _ => MaintOutcome::Abend(unexpected_data()),
        };
    }
    match edit_alphanum_reqd(&description) {
        Ok(()) => validated(code, description),
        Err(failure) => MaintOutcome::Failed(failure),
    }
}

/// State N showing the validated code and description (COTRTUPC-020).
fn validated(code: PicX<2>, description: String) -> MaintOutcome {
    MaintOutcome::Screen(TranTypeView::new(
        TtupState::ChangesOkNotConfirmed,
        code,
        shown(description),
        None,
    ))
}

/// `TTUP-OLD-DETAILS` for a step in `state`: the re-read row, or blanks when none was found. In
/// state R they are blanks whatever the re-read finds, as R follows a read that found nothing
/// (COTRTUPC-021).
fn old_values(state: TtupState, current: TypeLookup<'_>) -> Option<&Db2TransactionType> {
    match (state, current) {
        (TtupState::CreateNewRecord, _) | (_, TypeLookup::NotFound | TypeLookup::Error) => None,
        (_, TypeLookup::Found(row)) => Some(row),
    }
}

/// `1205-COMPARE-OLD-NEW`: whether the entered code and description equal the old values once
/// upper-cased, the descriptions also trimmed, with the same trimmed length (COTRTUPC-017).
///
/// `FUNCTION UPPER-CASE` folds the letters `a` to `z` alone, and `FUNCTION TRIM` removes spaces
/// alone, so a blank description, low-values, never equals a stored one. Trimmed texts have no
/// trailing spaces, so their equality includes the length test. Blank old values are spaces:
/// a code of two spaces and an empty description.
fn unchanged(code: &PicX<2>, description: &str, old: Option<&Db2TransactionType>) -> bool {
    let blank = PicX::<2>::spaces();
    let (old_code, old_description) =
        old.map_or((&blank, ""), |row| (row.tr_type(), row.tr_description()));
    code.as_bytes().eq_ignore_ascii_case(old_code.as_bytes())
        && description
            .trim_matches(' ')
            .eq_ignore_ascii_case(old_description.trim_matches(' '))
}

/// The no-change failure, leaving `state` (COTRTUPC-017).
const fn no_change(state: TtupState) -> MaintFailure {
    MaintFailure::input(NO_CHANGES_DETECTED, Some(MaintField::Description), state)
}

/// `WS-INVALID-KEY-PRESSED`, leaving `state` (COTRTUPC-042, COTRTUPC-043).
const fn invalid_key(state: TtupState) -> MaintFailure {
    MaintFailure::input(INVALID_KEY_PRESSED, None, state)
}

/// A key edit failure: the code in error and the fresh state, `TTUP-DETAILS-NOT-FETCHED`
/// (COTRTUPC-005 to COTRTUPC-007).
const fn code_failure(message: &'static str) -> MaintFailure {
    MaintFailure::input(
        message,
        Some(MaintField::TypeCode),
        TtupState::DetailsNotFetched,
    )
}

/// A description edit failure: the description in error and state E, `TTUP-CHANGES-NOT-OK`
/// (COTRTUPC-018, COTRTUPC-019).
const fn description_failure(message: &'static str) -> MaintFailure {
    MaintFailure::input(
        message,
        Some(MaintField::Description),
        TtupState::ChangesNotOk,
    )
}

/// The abend data of the `WHEN OTHER` of `2000-DECIDE-ACTION` as `ABEND-ROUTINE` sends it
/// (`COTRTUPC.cbl:1073-1080,1675-1698`; COTRTUPC-037).
fn unexpected_data() -> AbendData {
    AbendData::new(ABEND_CODE_9999, PROGRAM, "", ABEND_MSG_UNEXPECTED_DATA)
}

/// `TTUP-NEW-TTYP-TYPE` after `1150-STORE-MAP-IN-NEW`.
enum StoredCode {
    /// `*` alone, spaces or low-values: `LOW-VALUES` is stored.
    Blank,
    /// The trimmed code, left-justified and space-padded.
    Code(PicX<2>),
    /// A code with a character outside printable ASCII, which no [`PicX`] holds.
    Unreadable,
}

/// `1150-STORE-MAP-IN-NEW` for `TRTYPCDI` (`COTRTUPC.cbl:663-672`; COTRTUPC-004).
fn store_code(raw: &str) -> StoredCode {
    let field = fit(raw, CODE_WIDTH);
    if is_blank(&field) || field.trim_end_matches(' ') == "*" {
        return StoredCode::Blank;
    }
    PicX::from_text(field.trim_matches(' ')).map_or(StoredCode::Unreadable, StoredCode::Code)
}

/// `1150-STORE-MAP-IN-NEW` for `TRTYDSCI` (`COTRTUPC.cbl:676-683`; COTRTUPC-004): the 50
/// characters of `TTUP-NEW-TTYP-TYPE-DESC`, all low-values when blank.
fn store_description(raw: &str) -> String {
    let field = fit(raw, DESCRIPTION_WIDTH);
    if is_blank(&field) || field.trim_end_matches(' ') == "*" {
        return iter::repeat_n('\0', DESCRIPTION_WIDTH).collect();
    }
    fit(field.trim_matches(' '), DESCRIPTION_WIDTH)
}

/// `1230-EDIT-ALPHANUM-REQD` over the stored description (`COTRTUPC.cbl:849-899`;
/// COTRTUPC-018, COTRTUPC-019).
fn edit_alphanum_reqd(field: &str) -> Result<(), MaintFailure> {
    if is_blank(field) || field.trim_matches(' ').is_empty() {
        return Err(description_failure(DESCRIPTION_SUPPLIED));
    }
    // `INSPECT … CONVERTING` turns letters and digits into spaces; any character left after
    // `FUNCTION TRIM` is one the edit refuses.
    if field.chars().all(|c| c == ' ' || c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(description_failure(DESCRIPTION_ALPHANUMERIC))
    }
}

/// The stored description as the screen shows it: `None` for low-values.
fn shown(field: String) -> Option<String> {
    (!is_blank(&field)).then_some(field)
}

/// `FUNCTION NUMVAL` of a value `FUNCTION TEST-NUMVAL` accepts.
struct Numval {
    /// Whether the value is 0: every digit `0`.
    zero: bool,
    /// The last two digits of the integer part, `0`-filled: the value stored into an unsigned
    /// `PIC 9(02)` without `ROUNDED`, which drops the sign, the fraction and the high-order
    /// digits (COTRTUPC-008).
    units: [u8; 2],
}

/// `FUNCTION TEST-NUMVAL` and `FUNCTION NUMVAL` of `field` (COTRTUPC-006 to COTRTUPC-008):
/// `None` when the text is not `[spaces][+|-][spaces]digits[spaces]` or
/// `[spaces]digits[spaces][+|-|CR|DB][spaces]`, digits being at least one digit with at most one
/// decimal point. Within the two bytes of a code no `CR` or `DB` form has room for a digit.
fn numval(field: &[u8]) -> Option<Numval> {
    let body = trim_end_spaces(trim_start_spaces(field));
    let digits = match body.split_first() {
        Some((b'+' | b'-', rest)) => trim_start_spaces(rest),
        _ => strip_trailing_sign(body),
    };
    let mut point = false;
    let mut any_digit = false;
    let mut zero = true;
    let mut units = [b'0'; 2];
    for &byte in digits {
        match byte {
            b'.' if !point => point = true,
            b'0'..=b'9' => {
                any_digit = true;
                zero &= byte == b'0';
                if !point {
                    let [_, ones] = units;
                    units = [ones, byte];
                }
            }
            _ => return None,
        }
    }
    any_digit.then_some(Numval { zero, units })
}

/// `body` without a trailing sign and the spaces before it, or `body` when it has none.
fn strip_trailing_sign(body: &[u8]) -> &[u8] {
    TRAILING_SIGNS
        .iter()
        .find_map(|sign| body.strip_suffix(*sign))
        .map_or(body, trim_end_spaces)
}

/// `bytes` without leading spaces.
fn trim_start_spaces(mut bytes: &[u8]) -> &[u8] {
    while let [b' ', rest @ ..] = bytes {
        bytes = rest;
    }
    bytes
}

/// `bytes` without trailing spaces.
fn trim_end_spaces(mut bytes: &[u8]) -> &[u8] {
    while let [rest @ .., b' '] = bytes {
        bytes = rest;
    }
    bytes
}

/// `text` moved into a `PIC X(width)` field: its first `width` characters, padded with spaces.
fn fit(text: &str, width: usize) -> String {
    text.chars().chain(iter::repeat(' ')).take(width).collect()
}

/// `= SPACES OR LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(field: &str) -> bool {
    field.chars().all(|c| c == ' ') || field.chars().all(|c| c == '\0')
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;

    use super::TtupState as S;
    use super::{
        FailureClass, MaintEffect, MaintField, MaintInput, MaintOutcome, StoreResult, TtupAid,
        TtupState, TypeLookup, apply_result, check_pfkeys, decide, edit_description,
        edit_type_code, fetch, fetch_key, numval, receive_type_code, served_state,
    };
    use crate::messages::AbendData;
    use crate::records::db2_transaction_type::Db2TransactionType;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A failure as the response shows it: message, field, HTTP status and the state left.
    type ShownFailure = (&'static str, Option<MaintField>, u16, TtupState);

    /// A screen as the response shows it: state, code bytes, description, error row, info row.
    type ShownScreen = (
        TtupState,
        [u8; 2],
        Option<String>,
        Option<&'static str>,
        &'static str,
    );

    // The audited literals, written out apart from the module's own constants.
    const SEARCH: &str = "Enter transaction type to be maintained";
    const ADD_PROMPT: &str = "Press F05 to add. F12 to cancel";
    const NEW_DATA: &str = "Enter new transaction type details.";
    const CHANGES: &str = "Update transaction type details shown.";
    const VALIDATED: &str = "Changes validated.Press F5 to save";
    const COMMITTED: &str = "Changes committed to database";
    const UNSUCCESSFUL: &str = "Changes unsuccessful";
    const DELETE_PROMPT: &str = "Delete this record ? Press F4 to confirm";
    const DELETED: &str = "Delete successful.";
    const NOT_FOUND: &str = "No record found for this key in database";
    const NO_CHANGE: &str = "No change detected with respect to values fetched.";
    const LOCKED: &str = "Could not lock record for update";
    const INVALID_KEY: &str = "Invalid key pressed";
    const CODE_SUPPLIED: &str = "Tran Type code must be supplied.";
    const CODE_NUMERIC: &str = "Tran Type code must be numeric.";
    const CODE_ZERO: &str = "Tran Type code must not be zero.";
    const DESC_SUPPLIED: &str = "Transaction Desc must be supplied.";
    const DESC_ALPHANUM: &str = "Transaction Desc can have numbers or alphabets only.";
    const FETCH_ERROR: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";
    const UPDATE_ERROR: &str = "Error updating: TRANSACTION_TYPE Table. SQLCODE:";
    const INSERT_ERROR: &str = "Error inserting record into: TRANSACTION_TYPE Table. SQLCODE:";
    const DELETE_RESTRICTED: &str = "Please delete associated child records first:SQLCODE :";
    const DELETE_FAILED: &str = "Delete failed with message:SQLCODE :";

    /// Every state, in declaration order.
    const STATES: [TtupState; 16] = [
        S::DetailsNotFetched,
        S::InvalidSearchKeys,
        S::DetailsNotFound,
        S::ShowDetails,
        S::CreateNewRecord,
        S::ReviewNewRecord,
        S::ConfirmDelete,
        S::StartDelete,
        S::DeleteDone,
        S::DeleteFailed,
        S::ChangesNotOk,
        S::ChangesOkNotConfirmed,
        S::ChangesOkayedLockError,
        S::ChangesOkayedButFailed,
        S::ChangesOkayedAndDone,
        S::ChangesBackedOut,
    ];

    /// The seeded row `01`, `PURCHASE` (`DB2LTTYP.ctl:17`).
    fn purchase() -> Result<Db2TransactionType, Box<dyn StdError>> {
        row("01", "PURCHASE")
    }

    fn row(code: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            PicX::<2>::from_text(code)?,
            description.to_owned(),
        )?)
    }

    fn code(text: &str) -> Result<PicX<2>, Box<dyn StdError>> {
        Ok(PicX::<2>::from_text(text)?)
    }

    /// `text` space-padded to the 50 bytes of `TTUP-NEW-TTYP-TYPE-DESC`.
    fn padded(text: &str) -> String {
        format!("{text:<50}")
    }

    fn input<'a>(type_code: &'a str, description: &'a str) -> MaintInput<'a> {
        MaintInput {
            type_code,
            description,
        }
    }

    fn failure(outcome: &MaintOutcome) -> Option<ShownFailure> {
        match outcome {
            MaintOutcome::Failed(failure) => Some((
                failure.message(),
                failure.field(),
                failure.class().http_status(),
                failure.state(),
            )),
            _ => None,
        }
    }

    fn screen(outcome: &MaintOutcome) -> Option<ShownScreen> {
        match outcome {
            MaintOutcome::Screen(view) => Some((
                view.state(),
                *view.code().as_bytes(),
                view.description().map(str::to_owned),
                view.message(),
                view.info_message(),
            )),
            _ => None,
        }
    }

    fn unexpected_data() -> MaintOutcome {
        MaintOutcome::Abend(AbendData::new(
            "9999",
            "COTRTUPC",
            "",
            "UNEXPECTED DATA SCENARIO",
        ))
    }

    // audit: COTRTUPC-042
    // audit: COTRTUPC-043
    #[test]
    fn check_pfkeys_accepts_the_keys_each_state_serves() {
        let accepted: [(TtupAid, &[TtupState]); 3] = [
            (TtupAid::Pf4, &[S::ShowDetails, S::ConfirmDelete]),
            (
                TtupAid::Pf5,
                &[
                    S::ChangesOkNotConfirmed,
                    S::DetailsNotFound,
                    S::ConfirmDelete,
                    S::StartDelete,
                    S::DeleteDone,
                    S::DeleteFailed,
                ],
            ),
            (
                TtupAid::Pf12,
                &[
                    S::ChangesOkNotConfirmed,
                    S::ShowDetails,
                    S::DetailsNotFound,
                    S::ConfirmDelete,
                    S::CreateNewRecord,
                ],
            ),
        ];
        for state in STATES {
            let enter = check_pfkeys(TtupAid::Enter, state);
            if state == S::ConfirmDelete {
                assert!(enter.is_err());
            } else {
                assert_eq!(enter, Ok(TtupAid::Enter), "{state:?}");
            }
            for (aid, states) in accepted {
                match check_pfkeys(aid, state) {
                    Ok(key) => {
                        assert!(states.contains(&state), "{aid:?} in {state:?}");
                        assert_eq!(key, aid);
                    }
                    Err(refused) => {
                        assert!(!states.contains(&state), "{aid:?} in {state:?}");
                        assert_eq!(refused.message(), INVALID_KEY);
                        assert_eq!(refused.field(), None);
                        assert_eq!(refused.class(), FailureClass::Input);
                        assert_eq!(refused.state(), state);
                        assert_eq!(refused.to_string(), INVALID_KEY);
                    }
                }
            }
        }
    }

    // audit: COTRTUPC-004
    // audit: COTRTUPC-005
    #[test]
    fn receive_type_code_stores_the_trimmed_code_and_refuses_a_blank() -> TestResult {
        for (raw, stored) in [
            ("9", "9 "),
            (" 9", "9 "),
            ("AB", "AB"),
            (" *", "* "),
            ("123", "12"),
        ] {
            assert_eq!(receive_type_code(raw)?, code(stored)?, "{raw:?}");
        }
        for raw in ["", " ", "  ", "*", "* ", "\0\0"] {
            let refused = receive_type_code(raw).err().ok_or("blank accepted")?;
            assert_eq!(refused.message(), CODE_SUPPLIED, "{raw:?}");
            assert_eq!(refused.field(), Some(MaintField::TypeCode));
            assert_eq!(refused.class().http_status(), 400);
            assert_eq!(refused.state(), S::DetailsNotFetched);
        }
        for raw in ["é", "1\0", "\t1"] {
            let refused = receive_type_code(raw).err().ok_or("unreadable accepted")?;
            assert_eq!(refused.message(), CODE_NUMERIC, "{raw:?}");
        }
        Ok(())
    }

    // audit: COTRTUPC-005
    // audit: COTRTUPC-006
    // audit: COTRTUPC-007
    #[test]
    fn edit_type_code_runs_the_key_edits_in_order() -> TestResult {
        for (raw, message) in [
            ("", CODE_SUPPLIED),
            ("*", CODE_SUPPLIED),
            ("AB", CODE_NUMERIC),
            ("1A", CODE_NUMERIC),
            ("-", CODE_NUMERIC),
            (".", CODE_NUMERIC),
            ("CR", CODE_NUMERIC),
            ("+-", CODE_NUMERIC),
            ("é", CODE_NUMERIC),
            ("00", CODE_ZERO),
            ("0", CODE_ZERO),
            ("-0", CODE_ZERO),
            ("0.", CODE_ZERO),
            (".0", CODE_ZERO),
        ] {
            let refused = edit_type_code(raw).err().ok_or("edit passed")?;
            assert_eq!(
                (refused.message(), refused.field(), refused.state()),
                (message, Some(MaintField::TypeCode), S::DetailsNotFetched),
                "{raw:?}"
            );
            assert_eq!(refused.class(), FailureClass::Input);
        }
        Ok(())
    }

    // audit: COTRTUPC-008
    #[test]
    fn edit_type_code_keeps_the_two_digits_of_the_integer_part() -> TestResult {
        for (raw, normalised) in [
            ("1", "01"),
            ("01", "01"),
            ("+1", "01"),
            ("-1", "01"),
            ("1-", "01"),
            ("1+", "01"),
            ("1.", "01"),
            (" 7", "07"),
            ("9 ", "09"),
            ("99", "99"),
            ("123", "12"),
            (".5", "00"),
        ] {
            assert_eq!(edit_type_code(raw)?, code(normalised)?, "{raw:?}");
        }
        Ok(())
    }

    #[test]
    fn numval_follows_the_argument_syntax_of_both_formats() {
        for (text, expected) in [
            (&b"12"[..], Some((false, *b"12"))),
            (b" 1 ", Some((false, *b"01"))),
            (b"+ 1", Some((false, *b"01"))),
            (b"1 -", Some((false, *b"01"))),
            (b"345CR", Some((false, *b"45"))),
            (b"3 DB", Some((false, *b"03"))),
            (b"1.5", Some((false, *b"01"))),
            (b"0.0", Some((true, *b"00"))),
            (b"00.07", Some((false, *b"00"))),
            (b"", None),
            (b"   ", None),
            (b".", None),
            (b"1..", None),
            (b"1.2.", None),
            (b"+1-", None),
            (b"--1", None),
            (b"1 2", None),
            (b"cr", None),
            (b"1cr", None),
        ] {
            let parsed = numval(text).map(|value| (value.zero, value.units));
            assert_eq!(parsed, expected, "{}", text.escape_ascii());
        }
    }

    // audit: COTRTUPC-012
    #[test]
    fn fetch_key_skips_the_key_edits_in_state_x_only() -> TestResult {
        for (raw, kept) in [("AB", "AB"), ("00", "00"), ("9", "9 "), (" 5", "5 ")] {
            assert_eq!(fetch_key(raw, S::DetailsNotFound)?, code(kept)?, "{raw:?}");
        }
        for prior in [S::DetailsNotFetched, S::InvalidSearchKeys] {
            assert_eq!(fetch_key("9", prior)?, code("09")?);
            let refused = fetch_key("AB", prior).err().ok_or("AB passed")?;
            assert_eq!(refused.message(), CODE_NUMERIC);
        }
        // A blank code still gets the blank test in state X.
        for raw in ["", "*"] {
            let refused = fetch_key(raw, S::DetailsNotFound)
                .err()
                .ok_or("blank passed")?;
            assert_eq!(refused.message(), CODE_SUPPLIED);
        }
        Ok(())
    }

    // audit: COTRTUPC-004
    // audit: COTRTUPC-018
    // audit: COTRTUPC-019
    // audit: COTRTUPC-023
    #[test]
    fn edit_description_trims_pads_and_edits() -> TestResult {
        for (raw, stored) in [
            ("New type", "New type"),
            ("  Leading and trailing  ", "Leading and trailing"),
            ("lower case 42", "lower case 42"),
        ] {
            let value = edit_description(raw)?;
            assert_eq!(value, padded(stored), "{raw:?}");
            assert_eq!(value.len(), 50);
            // The VARCHAR(50) rule of the row type holds for every edited value.
            let written = Db2TransactionType::new(code("05")?, value.clone())?;
            assert_eq!(written.tr_description(), value);
        }
        // The field keeps its first 50 characters, as TRTYDSCI PIC X(50) does.
        let long = "A".repeat(60);
        assert_eq!(edit_description(&long)?, "A".repeat(50));
        for (raw, message) in [
            ("", DESC_SUPPLIED),
            ("   ", DESC_SUPPLIED),
            ("*", DESC_SUPPLIED),
            ("\0\0\0", DESC_ALPHANUM),
            ("*A", DESC_ALPHANUM),
            ("Buy-now", DESC_ALPHANUM),
            ("Café", DESC_ALPHANUM),
            ("Tab\there", DESC_ALPHANUM),
        ] {
            let refused = edit_description(raw).err().ok_or("edit passed")?;
            assert_eq!(
                (refused.message(), refused.field(), refused.state()),
                (message, Some(MaintField::Description), S::ChangesNotOk),
                "{raw:?}"
            );
            assert_eq!(refused.class().http_status(), 400);
        }
        Ok(())
    }

    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTUPC-011
    #[test]
    fn fetch_answers_a_row_no_row_and_a_failed_read() -> TestResult {
        let key = code("01")?;
        let stored = row("01", "PURCHASE  ")?;
        let found = fetch(&key, TypeLookup::Found(&stored))?;
        assert_eq!(found.state(), S::ShowDetails);
        assert_eq!(found.code(), &key);
        // The stored text up to its stored length, trailing spaces kept.
        assert_eq!(found.description(), Some("PURCHASE  "));
        assert_eq!(found.message(), None);
        assert_eq!(found.info_message(), SEARCH);

        let missing = fetch(&code("AB")?, TypeLookup::NotFound)?;
        assert_eq!(missing.state(), S::DetailsNotFound);
        assert_eq!(missing.code(), &code("AB")?);
        assert_eq!(missing.description(), None);
        assert_eq!(missing.message(), Some(NOT_FOUND));
        assert_eq!(missing.info_message(), ADD_PROMPT);

        let refused = fetch(&key, TypeLookup::Error)
            .err()
            .ok_or("failed read answered")?;
        assert_eq!(
            (refused.message(), refused.field(), refused.state()),
            (FETCH_ERROR, Some(MaintField::TypeCode), S::DetailsNotFound)
        );
        assert_eq!(refused.class(), FailureClass::Store);
        assert_eq!(refused.class().http_status(), 500);
        Ok(())
    }

    // audit: COTRTUPC-016
    // audit: COTRTUPC-021
    // audit: COTRTUPC-024
    // audit: COTRTUPC-032
    #[test]
    fn served_state_follows_the_endpoint_mapping() -> TestResult {
        let stored = purchase()?;
        let lookups = [
            TypeLookup::Found(&stored),
            TypeLookup::NotFound,
            TypeLookup::Error,
        ];
        for current in lookups {
            assert_eq!(served_state(TtupAid::Pf4, current), S::ConfirmDelete);
            assert_eq!(
                served_state(TtupAid::Pf5, current),
                S::ChangesOkNotConfirmed
            );
        }
        for aid in [TtupAid::Enter, TtupAid::Pf12] {
            assert_eq!(
                served_state(aid, TypeLookup::Found(&stored)),
                S::ShowDetails
            );
            assert_eq!(served_state(aid, TypeLookup::NotFound), S::CreateNewRecord);
            assert_eq!(served_state(aid, TypeLookup::Error), S::ShowDetails);
        }
        Ok(())
    }

    // audit: COTRTUPC-043
    #[test]
    fn decide_answers_f12_and_refused_keys_with_invalid_key() -> TestResult {
        let stored = purchase()?;
        let typed = input("01", "Changed");
        for state in [
            S::ShowDetails,
            S::ChangesOkNotConfirmed,
            S::ConfirmDelete,
            S::CreateNewRecord,
        ] {
            let outcome = decide(TtupAid::Pf12, state, &typed, TypeLookup::Found(&stored));
            assert_eq!(
                failure(&outcome),
                Some((INVALID_KEY, None, 400, state)),
                "{state:?}"
            );
        }
        for (aid, state) in [
            (TtupAid::Enter, S::ConfirmDelete),
            (TtupAid::Pf4, S::DetailsNotFound),
            (TtupAid::Pf4, S::ChangesOkNotConfirmed),
            (TtupAid::Pf5, S::ShowDetails),
            (TtupAid::Pf5, S::CreateNewRecord),
        ] {
            let outcome = decide(aid, state, &typed, TypeLookup::Found(&stored));
            assert_eq!(failure(&outcome), Some((INVALID_KEY, None, 400, state)));
        }
        Ok(())
    }

    // audit: COTRTUPC-030
    // audit: COTRTUPC-039
    #[test]
    fn decide_restarts_the_ended_states_as_the_fresh_screen() -> TestResult {
        let stored = purchase()?;
        let fresh = Some((S::DetailsNotFetched, *b"  ", None, None, SEARCH));
        let typed = input("01", "Changed");
        for state in [
            S::ChangesOkayedAndDone,
            S::ChangesOkayedLockError,
            S::ChangesOkayedButFailed,
            S::DeleteDone,
            S::DeleteFailed,
        ] {
            // Any key, one the state refuses included, gives the fresh screen.
            for aid in [TtupAid::Enter, TtupAid::Pf4, TtupAid::Pf5] {
                let outcome = decide(aid, state, &typed, TypeLookup::Found(&stored));
                assert_eq!(screen(&outcome), fresh, "{aid:?} in {state:?}");
            }
        }
        let blank_old = decide(
            TtupAid::Enter,
            S::ChangesBackedOut,
            &typed,
            TypeLookup::NotFound,
        );
        assert_eq!(screen(&blank_old), fresh);
        let unknown_old = decide(
            TtupAid::Enter,
            S::ChangesBackedOut,
            &typed,
            TypeLookup::Error,
        );
        assert_eq!(
            failure(&unknown_old),
            Some(("", None, 500, S::ChangesBackedOut))
        );
        Ok(())
    }

    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTUPC-011
    // audit: COTRTUPC-012
    #[test]
    fn decide_serves_enter_in_the_fresh_and_x_states_as_the_fetch() -> TestResult {
        let stored = purchase()?;
        let found = decide(
            TtupAid::Enter,
            S::DetailsNotFetched,
            &input("1", ""),
            TypeLookup::Found(&stored),
        );
        assert_eq!(
            screen(&found),
            Some((
                S::ShowDetails,
                *b"01",
                Some("PURCHASE".to_owned()),
                None,
                SEARCH
            ))
        );
        let missing = decide(
            TtupAid::Enter,
            S::InvalidSearchKeys,
            &input("+8", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            screen(&missing),
            Some((
                S::DetailsNotFound,
                *b"08",
                None,
                Some(NOT_FOUND),
                ADD_PROMPT
            ))
        );
        let kept = decide(
            TtupAid::Enter,
            S::DetailsNotFound,
            &input("AB", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            screen(&kept),
            Some((
                S::DetailsNotFound,
                *b"AB",
                None,
                Some(NOT_FOUND),
                ADD_PROMPT
            ))
        );
        let failed = decide(
            TtupAid::Enter,
            S::DetailsNotFetched,
            &input("01", ""),
            TypeLookup::Error,
        );
        assert_eq!(
            failure(&failed),
            Some((
                FETCH_ERROR,
                Some(MaintField::TypeCode),
                500,
                S::DetailsNotFound
            ))
        );
        let refused = decide(
            TtupAid::Enter,
            S::DetailsNotFetched,
            &input("AB", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            failure(&refused),
            Some((
                CODE_NUMERIC,
                Some(MaintField::TypeCode),
                400,
                S::DetailsNotFetched
            ))
        );
        Ok(())
    }

    // audit: COTRTUPC-016
    // audit: COTRTUPC-017
    #[test]
    fn decide_enter_with_a_row_finds_no_change_by_case_and_trim() -> TestResult {
        let stored = purchase()?;
        let current = TypeLookup::Found(&stored);
        for (state, left) in [
            (S::ShowDetails, S::ShowDetails),
            (S::ChangesNotOk, S::ChangesNotOk),
            (S::ChangesBackedOut, S::ChangesNotOk),
        ] {
            for typed in [
                input("01", "PURCHASE"),
                input("01", "purchase"),
                input("1", "  Purchase  "),
            ] {
                let outcome = decide(TtupAid::Enter, state, &typed, current);
                assert_eq!(
                    failure(&outcome),
                    Some((NO_CHANGE, Some(MaintField::Description), 400, left)),
                    "{state:?} {typed:?}"
                );
            }
        }
        // A stored code the key edits refuse gets their message and the fresh state.
        let odd = row("AB", "ODD")?;
        let outcome = decide(
            TtupAid::Enter,
            S::ShowDetails,
            &input("AB", "ODD"),
            TypeLookup::Found(&odd),
        );
        assert_eq!(
            failure(&outcome),
            Some((
                CODE_NUMERIC,
                Some(MaintField::TypeCode),
                400,
                S::DetailsNotFetched
            ))
        );
        Ok(())
    }

    // audit: COTRTUPC-016
    // audit: COTRTUPC-018
    // audit: COTRTUPC-019
    // audit: COTRTUPC-020
    #[test]
    fn decide_enter_with_a_change_runs_the_description_edits() -> TestResult {
        let stored = purchase()?;
        let current = TypeLookup::Found(&stored);
        let outcome = decide(
            TtupAid::Enter,
            S::ShowDetails,
            &input("01", " Purchases 2 "),
            current,
        );
        assert_eq!(
            screen(&outcome),
            Some((
                S::ChangesOkNotConfirmed,
                *b"01",
                Some(padded("Purchases 2")),
                None,
                VALIDATED
            ))
        );
        for (description, message) in [
            ("", DESC_SUPPLIED),
            ("*", DESC_SUPPLIED),
            ("Buy-now", DESC_ALPHANUM),
        ] {
            let outcome = decide(
                TtupAid::Enter,
                S::ChangesBackedOut,
                &input("01", description),
                current,
            );
            assert_eq!(
                failure(&outcome),
                Some((message, Some(MaintField::Description), 400, S::ChangesNotOk)),
                "{description:?}"
            );
        }
        // The key edits normalise a one-digit stored code, which then differs from the row.
        let short = row("9 ", "PURCHASE")?;
        let outcome = decide(
            TtupAid::Enter,
            S::ShowDetails,
            &input("9", "PURCHASE"),
            TypeLookup::Found(&short),
        );
        assert_eq!(
            screen(&outcome),
            Some((
                S::ChangesOkNotConfirmed,
                *b"09",
                Some(padded("PURCHASE")),
                None,
                VALIDATED
            ))
        );
        Ok(())
    }

    // audit: COTRTUPC-021
    #[test]
    fn decide_enter_without_a_row_skips_the_key_edits_and_always_finds_a_change() -> TestResult {
        let stored = purchase()?;
        for (state, current) in [
            (S::CreateNewRecord, TypeLookup::NotFound),
            (S::ChangesNotOk, TypeLookup::NotFound),
            (S::ShowDetails, TypeLookup::NotFound),
            // State R compares with blanks whatever the re-read finds.
            (S::CreateNewRecord, TypeLookup::Found(&stored)),
        ] {
            let outcome = decide(TtupAid::Enter, state, &input("01", "PURCHASE"), current);
            assert_eq!(
                screen(&outcome),
                Some((
                    S::ChangesOkNotConfirmed,
                    *b"01",
                    Some(padded("PURCHASE")),
                    None,
                    VALIDATED
                )),
                "{state:?}"
            );
        }
        let unedited = decide(
            TtupAid::Enter,
            S::CreateNewRecord,
            &input("AB", "New"),
            TypeLookup::NotFound,
        );
        assert_eq!(
            screen(&unedited),
            Some((
                S::ChangesOkNotConfirmed,
                *b"AB",
                Some(padded("New")),
                None,
                VALIDATED
            ))
        );
        let blank = decide(
            TtupAid::Enter,
            S::CreateNewRecord,
            &input("05", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            failure(&blank),
            Some((
                DESC_SUPPLIED,
                Some(MaintField::Description),
                400,
                S::ChangesNotOk
            ))
        );
        let no_code = decide(
            TtupAid::Enter,
            S::CreateNewRecord,
            &input("", "New"),
            TypeLookup::NotFound,
        );
        assert_eq!(
            failure(&no_code),
            Some((
                CODE_SUPPLIED,
                Some(MaintField::TypeCode),
                400,
                S::DetailsNotFetched
            ))
        );
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    #[test]
    fn decide_pf5_in_state_n_returns_the_update_of_the_edited_values() -> TestResult {
        let stored = purchase()?;
        for current in [TypeLookup::NotFound, TypeLookup::Found(&stored)] {
            let outcome = decide(
                TtupAid::Pf5,
                S::ChangesOkNotConfirmed,
                &input("05", "  New type "),
                current,
            );
            assert_eq!(
                outcome,
                MaintOutcome::Execute(MaintEffect::Update {
                    code: code("05")?,
                    description: padded("New type"),
                })
            );
        }
        for (typed, shown) in [
            (
                input("05", ""),
                (
                    DESC_SUPPLIED,
                    Some(MaintField::Description),
                    400,
                    S::ChangesNotOk,
                ),
            ),
            (
                input("05", "a+b"),
                (
                    DESC_ALPHANUM,
                    Some(MaintField::Description),
                    400,
                    S::ChangesNotOk,
                ),
            ),
            (
                input("", "New"),
                (
                    CODE_SUPPLIED,
                    Some(MaintField::TypeCode),
                    400,
                    S::DetailsNotFetched,
                ),
            ),
            (
                input("*", "New"),
                (
                    CODE_SUPPLIED,
                    Some(MaintField::TypeCode),
                    400,
                    S::DetailsNotFetched,
                ),
            ),
        ] {
            let outcome = decide(
                TtupAid::Pf5,
                S::ChangesOkNotConfirmed,
                &typed,
                TypeLookup::NotFound,
            );
            assert_eq!(failure(&outcome), Some(shown), "{typed:?}");
        }
        Ok(())
    }

    // audit: COTRTUPC-031
    // audit: COTRTUPC-032
    #[test]
    fn decide_pf4_confirms_in_state_s_and_deletes_in_state_9() -> TestResult {
        let stored = purchase()?;
        let typed = input("01", "ignored");
        let delete = decide(
            TtupAid::Pf4,
            S::ConfirmDelete,
            &typed,
            TypeLookup::Found(&stored),
        );
        assert_eq!(
            delete,
            MaintOutcome::Execute(MaintEffect::Delete { code: code("01")? })
        );
        // A row gone since the fetch is still deleted by its code; the store reports no row.
        let gone = decide(TtupAid::Pf4, S::ConfirmDelete, &typed, TypeLookup::NotFound);
        assert_eq!(
            gone,
            MaintOutcome::Execute(MaintEffect::Delete { code: code("01")? })
        );
        let blank = decide(
            TtupAid::Pf4,
            S::ConfirmDelete,
            &input("", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            failure(&blank),
            Some((
                CODE_SUPPLIED,
                Some(MaintField::TypeCode),
                400,
                S::DetailsNotFetched
            ))
        );

        let confirm = decide(
            TtupAid::Pf4,
            S::ShowDetails,
            &typed,
            TypeLookup::Found(&stored),
        );
        assert_eq!(
            screen(&confirm),
            Some((
                S::ConfirmDelete,
                *b"01",
                Some("PURCHASE".to_owned()),
                None,
                DELETE_PROMPT
            ))
        );
        let without_row = decide(TtupAid::Pf4, S::ShowDetails, &typed, TypeLookup::NotFound);
        assert_eq!(
            screen(&without_row),
            Some((S::ConfirmDelete, *b"  ", None, None, DELETE_PROMPT))
        );
        Ok(())
    }

    // audit: COTRTUPC-015
    // audit: COTRTUPC-022
    #[test]
    fn decide_pf5_in_state_x_and_enter_in_state_n_write_nothing() -> TestResult {
        let stored = purchase()?;
        let add = decide(
            TtupAid::Pf5,
            S::DetailsNotFound,
            &input("07", "typed"),
            TypeLookup::NotFound,
        );
        assert_eq!(
            screen(&add),
            Some((S::CreateNewRecord, *b"07", None, None, NEW_DATA))
        );
        let blank = decide(
            TtupAid::Pf5,
            S::DetailsNotFound,
            &input(" ", ""),
            TypeLookup::NotFound,
        );
        assert_eq!(
            failure(&blank),
            Some((
                CODE_SUPPLIED,
                Some(MaintField::TypeCode),
                400,
                S::DetailsNotFetched
            ))
        );

        let current = TypeLookup::Found(&stored);
        let again = decide(
            TtupAid::Enter,
            S::ChangesOkNotConfirmed,
            &input("01", "Renamed"),
            current,
        );
        assert_eq!(
            screen(&again),
            Some((
                S::ChangesOkNotConfirmed,
                *b"01",
                Some(padded("Renamed")),
                None,
                VALIDATED
            ))
        );
        let blank_description = decide(
            TtupAid::Enter,
            S::ChangesOkNotConfirmed,
            &input("01", ""),
            current,
        );
        assert_eq!(
            screen(&blank_description),
            Some((S::ChangesOkNotConfirmed, *b"01", None, None, VALIDATED))
        );
        let same = decide(
            TtupAid::Enter,
            S::ChangesOkNotConfirmed,
            &input("01", "PURCHASE"),
            current,
        );
        assert_eq!(
            failure(&same),
            Some((
                NO_CHANGE,
                Some(MaintField::Description),
                400,
                S::ChangesOkNotConfirmed
            ))
        );
        let no_code = decide(
            TtupAid::Enter,
            S::ChangesOkNotConfirmed,
            &input("", "x"),
            current,
        );
        assert_eq!(failure(&no_code).map(|shown| shown.0), Some(CODE_SUPPLIED));
        Ok(())
    }

    // audit: COTRTUPC-035
    // audit: COTRTUPC-036
    // audit: COTRTUPC-037
    #[test]
    fn decide_abends_where_no_branch_serves_the_state() -> TestResult {
        let stored = purchase()?;
        let current = TypeLookup::Found(&stored);
        let same = input("01", "PURCHASE");
        for (aid, state) in [
            (TtupAid::Enter, S::StartDelete),
            (TtupAid::Pf5, S::StartDelete),
            (TtupAid::Pf5, S::ConfirmDelete),
            (TtupAid::Enter, S::ReviewNewRecord),
        ] {
            assert_eq!(
                decide(aid, state, &same, current),
                unexpected_data(),
                "{aid:?} {state:?}"
            );
        }
        // A change found in those states leaves them before 2000-DECIDE-ACTION.
        let changed = decide(
            TtupAid::Pf5,
            S::ConfirmDelete,
            &input("01", "Other"),
            current,
        );
        assert_eq!(
            screen(&changed),
            Some((
                S::ChangesOkNotConfirmed,
                *b"01",
                Some(padded("Other")),
                None,
                VALIDATED
            ))
        );
        Ok(())
    }

    // audit: COTRTUPC-038
    #[test]
    fn decide_fails_without_text_when_the_reread_fails() {
        for (aid, state) in [
            (TtupAid::Enter, S::ShowDetails),
            (TtupAid::Enter, S::CreateNewRecord),
            (TtupAid::Pf5, S::ChangesOkNotConfirmed),
            (TtupAid::Pf4, S::ConfirmDelete),
            (TtupAid::Pf4, S::ShowDetails),
            (TtupAid::Pf5, S::DetailsNotFound),
        ] {
            let outcome = decide(aid, state, &input("01", "New"), TypeLookup::Error);
            assert_eq!(
                failure(&outcome),
                Some(("", None, 500, state)),
                "{aid:?} {state:?}"
            );
        }
    }

    // audit: COTRTUPC-024
    // audit: COTRTUPC-026
    // audit: COTRTUPC-028
    // audit: COTRTUPC-029
    #[test]
    fn apply_result_answers_the_update() -> TestResult {
        let update = || -> Result<MaintEffect, Box<dyn StdError>> {
            Ok(MaintEffect::Update {
                code: code("05")?,
                description: padded("New type"),
            })
        };
        assert_eq!(
            screen(&apply_result(update()?, StoreResult::Done)),
            Some((
                S::ChangesOkayedAndDone,
                *b"05",
                Some(padded("New type")),
                None,
                COMMITTED
            ))
        );
        assert_eq!(
            apply_result(update()?, StoreResult::NoRow),
            MaintOutcome::Execute(MaintEffect::Insert {
                code: code("05")?,
                description: padded("New type"),
            })
        );
        let locked = apply_result(update()?, StoreResult::LockNotAvailable);
        assert_eq!(
            failure(&locked),
            Some((LOCKED, None, 500, S::ChangesOkayedLockError))
        );
        assert_eq!(S::ChangesOkayedLockError.info_message(), UNSUCCESSFUL);
        for result in [StoreResult::OtherError, StoreResult::ForeignKeyViolation] {
            let failed = apply_result(update()?, result);
            // The info row of state C says committed although nothing was written.
            assert_eq!(
                failure(&failed),
                Some((UPDATE_ERROR, None, 500, S::ChangesOkayedAndDone))
            );
        }
        Ok(())
    }

    // audit: COTRTUPC-026
    // audit: COTRTUPC-027
    #[test]
    fn apply_result_answers_the_insert() -> TestResult {
        let insert = || -> Result<MaintEffect, Box<dyn StdError>> {
            Ok(MaintEffect::Insert {
                code: code("AB")?,
                description: padded("Added"),
            })
        };
        assert_eq!(
            screen(&apply_result(insert()?, StoreResult::Done)),
            Some((
                S::ChangesOkayedAndDone,
                *b"AB",
                Some(padded("Added")),
                None,
                COMMITTED
            ))
        );
        for result in [
            StoreResult::NoRow,
            StoreResult::LockNotAvailable,
            StoreResult::ForeignKeyViolation,
            StoreResult::OtherError,
        ] {
            let failed = apply_result(insert()?, result);
            assert_eq!(
                failure(&failed),
                Some((INSERT_ERROR, None, 500, S::ChangesOkayedAndDone))
            );
        }
        Ok(())
    }

    // audit: COTRTUPC-032
    // audit: COTRTUPC-033
    // audit: COTRTUPC-034
    #[test]
    fn apply_result_answers_the_delete() -> TestResult {
        let delete = || -> Result<MaintEffect, Box<dyn StdError>> {
            Ok(MaintEffect::Delete { code: code("01")? })
        };
        assert_eq!(
            screen(&apply_result(delete()?, StoreResult::Done)),
            Some((S::DeleteDone, *b"01", None, None, DELETED))
        );
        // The RESTRICT foreign key refuses the delete: nothing is deleted and the state stays 8.
        let restricted = apply_result(delete()?, StoreResult::ForeignKeyViolation);
        assert_eq!(
            failure(&restricted),
            Some((DELETE_RESTRICTED, None, 500, S::StartDelete))
        );
        assert_eq!(S::StartDelete.info_message(), SEARCH);
        for result in [
            StoreResult::NoRow,
            StoreResult::LockNotAvailable,
            StoreResult::OtherError,
        ] {
            let failed = apply_result(delete()?, result);
            assert_eq!(
                failure(&failed),
                Some((
                    DELETE_FAILED,
                    Some(MaintField::TypeCode),
                    500,
                    S::DeleteFailed
                ))
            );
        }
        Ok(())
    }

    // audit: COTRTUPC-029
    // audit: COTRTUPC-033
    #[test]
    fn store_result_maps_the_sqlstates_the_program_names() {
        for (sqlstate, result) in [
            ("23503", StoreResult::ForeignKeyViolation),
            ("40P01", StoreResult::LockNotAvailable),
            ("55P03", StoreResult::LockNotAvailable),
            ("23505", StoreResult::OtherError),
            ("", StoreResult::OtherError),
        ] {
            assert_eq!(StoreResult::from_sqlstate(sqlstate), result, "{sqlstate:?}");
        }
    }

    // audit: COTRTUPC-045
    #[test]
    fn info_messages_and_group_levels_follow_the_state() {
        for (state, info, delete, changes, failed) in [
            (S::DetailsNotFetched, SEARCH, false, false, false),
            (S::InvalidSearchKeys, SEARCH, false, false, false),
            (S::DetailsNotFound, ADD_PROMPT, false, false, false),
            (S::ShowDetails, SEARCH, false, false, false),
            (S::CreateNewRecord, NEW_DATA, false, false, false),
            (S::ReviewNewRecord, SEARCH, false, false, false),
            (S::ConfirmDelete, DELETE_PROMPT, true, false, false),
            (S::StartDelete, SEARCH, true, false, false),
            (S::DeleteDone, DELETED, true, false, false),
            (S::DeleteFailed, UNSUCCESSFUL, true, false, false),
            (S::ChangesNotOk, CHANGES, false, true, false),
            (S::ChangesOkNotConfirmed, VALIDATED, false, true, false),
            (S::ChangesOkayedLockError, UNSUCCESSFUL, false, true, true),
            (S::ChangesOkayedButFailed, UNSUCCESSFUL, false, true, true),
            (S::ChangesOkayedAndDone, COMMITTED, false, false, false),
            (S::ChangesBackedOut, CHANGES, false, false, false),
        ] {
            assert_eq!(state.info_message(), info, "{state:?}");
            assert_eq!(state.is_delete_in_progress(), delete, "{state:?}");
            assert_eq!(state.is_changes_made(), changes, "{state:?}");
            assert_eq!(state.is_changes_failed(), failed, "{state:?}");
            // `WS-INFO-MSG PIC X(40)`.
            assert!(info.len() <= 40);
        }
    }

    #[test]
    fn every_error_row_fits_ws_return_msg() {
        for message in [
            NOT_FOUND,
            NO_CHANGE,
            LOCKED,
            INVALID_KEY,
            CODE_SUPPLIED,
            CODE_NUMERIC,
            CODE_ZERO,
            DESC_SUPPLIED,
            DESC_ALPHANUM,
            FETCH_ERROR,
            UPDATE_ERROR,
            INSERT_ERROR,
            DELETE_RESTRICTED,
            DELETE_FAILED,
        ] {
            // `WS-RETURN-MSG PIC X(75)`.
            assert!(message.len() <= 75, "{message}");
        }
        assert_eq!(FailureClass::Input.http_status(), 400);
        assert_eq!(FailureClass::Store.http_status(), 500);
    }
}
