//! COCRDUPC, the card update: the rules of endpoint 9, `GET /api/v1/cards/edit`, and endpoint
//! 10, `PUT /api/v1/cards/{card_ref}` (transaction `CCUP`, map `CCRDUPA`, route `/cards/update`;
//! AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COCRDUPC.md`: the key edits of
//! `1210-EDIT-ACCOUNT` and `1220-EDIT-CARD` and the card read of `9000-READ-DATA` (COCRDUPC-002,
//! COCRDUPC-003, COCRDUPC-010 to COCRDUPC-019), the change check and the detail edits of
//! `1200-EDIT-MAP-INPUTS` and `1230-EDIT-NAME` to `1260-EDIT-EXPIRY-YEAR` (COCRDUPC-009,
//! COCRDUPC-020 to COCRDUPC-032), the state machine of `2000-DECIDE-ACTION` (COCRDUPC-007,
//! COCRDUPC-034, COCRDUPC-035, COCRDUPC-041), the save of `9200-WRITE-PROCESSING` and
//! `9300-CHECK-CHANGE-IN-REC` (COCRDUPC-036 to COCRDUPC-040, COCRDUPC-043) and the snapshot
//! that stands for `CCUP-OLD-DETAILS` (COCRDUPC-044). The `client` rows belong to the screen:
//! entry, F3, F12 and the transfers (COCRDUPC-001, COCRDUPC-004 to COCRDUPC-006), the key
//! mapping (COCRDUPC-008), the zero fill of the month and year (COCRDUPC-033) and the field
//! attributes and colours (COCRDUPC-045, COCRDUPC-046).
//!
//! # Flow
//!
//! Endpoint 9: `api::services::cocrdupc` calls [`edit_keys`] on the raw query, reads the card by
//! the [`CardKey`] it returns, maps the store's answer into a [`CardLookup`] and calls [`fetch`],
//! which yields the [`CardUpdateView`] or the failure.
//!
//! Endpoint 10: the service builds the echoed [`CardSnapshot`] with [`CardSnapshot::from_fields`]
//! and the raw [`CardUpdateInput`], and calls [`decide`]. A [`Decision::Respond`] is answered as
//! it stands, with nothing read or written. A [`Decision::Save`] carries the
//! [`CardSaveEffects`]; the service then reads the card that `card_ref` names for update and
//! answers [`CardUpdateFailure::lock_failed`] when it cannot, runs
//! [`CardSaveEffects::check_change_in_rec`] on the record it holds, rewrites the card with
//! [`CardSaveEffects::rewritten`] and answers [`CardSaveEffects::committed`] or
//! [`CardUpdateFailure::rewrite_failed`]. This module does no I/O.
//!
//! # State
//!
//! The COBOL keeps `CCUP-CHANGE-ACTION` and `CCUP-OLD-DETAILS` in the COMMAREA between tasks.
//! Endpoint 10 is stateless, so both travel in the [`CardSnapshot`]: the state after each
//! response and the values fetched, without the card number and the CVV. The client echoes it
//! unchanged and it grants nothing: a save re-runs the change check and the edits on the fields
//! received and compares the card read for update with it (COCRDUPC-037, COCRDUPC-041,
//! COCRDUPC-044, `X-COCRDUPC-1`).
//!
//! # Quirks kept
//!
//! - The card is read by its number alone and shown under the account entered, and the save
//!   writes that account into the card, whatever account it held (COCRDUPC-016, COCRDUPC-038,
//!   `X-COCRDUPC-2`).
//! - The fetched name is upper-cased, so an untouched name is written back upper-cased, and the
//!   change check compares both sides upper-cased, so a change of letter case alone is never
//!   detected (COCRDUPC-017, COCRDUPC-022, COCRDUPC-037).
//! - A field received blank or as `*` becomes low-values, which equal no fetched value, so it
//!   always reads as a change (COCRDUPC-009).
//! - Every detail is edited, an untouched one included: a card whose stored name fails the
//!   alphabet edit can be saved only with a new name (COCRDUPC-026).
//!
//! # Overrides
//!
//! - Cardholder Data Redaction (`E32`): the card number leaves only as [`CardNumber`], whose
//!   `Debug`, `Display` and `Serialize` are the redacted form, and the card is identified by its
//!   [`CardRef`]; no type here holds a CVV outside a [`CardRecord`] (COCRDUPC-042,
//!   COCRDUPC-043).
//! - The change check of `9300-CHECK-CHANGE-IN-REC` drops its CVV term, because no snapshot
//!   carries a CVV (COCRDUPC-037, `X-COCRDUPC-4`).
//! - Typed Dates and Timestamps: the expiry date is a typed date in the view, the snapshot and
//!   the rewritten record; the month and year are raw screen text that the edits own. A save
//!   whose year, month and stored day name no calendar date fails as a rewrite failure, which is
//!   no parity result (COCRDUPC-038, `X-COCRDUPC-3`).
//!
//! # Halts
//!
//! - `HALTED(H4)`: the `RESP` and `RESP2` fields of the file-error text keep spaces; they never
//!   carry a key value or database text (COCRDUPC-019, COCRDUPC-047).
//! - Halt candidate `X-COCRDUPC-H1`: every entry is treated as a fresh one, and the rewrite
//!   keeps the stored `FILLER` (COCRDUPC-002, COCRDUPC-038).
//! - Halt candidate `X-COCRDUPC-H2`: a month or year holding a non-digit fails its own edit, an
//!   account or card of zero-nibble bytes fails the class test, and the rewrite keeps the stored
//!   CVV; none of these is a parity result (COCRDUPC-011, COCRDUPC-013, COCRDUPC-030,
//!   COCRDUPC-032, COCRDUPC-038).
//!
//! # Not ported
//!
//! The seven 88-level texts the program declares and never sets are never returned, and the
//! `WHEN OTHER` abend of `2000-DECIDE-ACTION` has no port: [`CardUpdateState`] holds only the
//! codes the program sets, and an echoed state it has no code for is refused by the request DTO
//! before this module is reached. `ABEND-ROUTINE` returns nothing (`X-COCRDUPC-8`).
//!
//! # Totality
//!
//! Every function takes text of any length or encoding and returns a value or a
//! [`CardUpdateFailure`]; nothing indexes, slices or panics, and no failure holds any part of
//! the input (Total Parsers Over Untrusted Input, Cardholder Data Redaction).

use core::fmt;
use core::ops::RangeInclusive;

use chrono::{Datelike, NaiveDate};
use copybook::{PicDate, PicU, PicX};

use super::cocrdlic::CardRef;
use super::cocrdslc::{self, DetailFailure, DetailField};
use crate::records::card::CardRecord;
use crate::sensitive::CardNumber;

/// The search-key texts of endpoint 9 and the card-read failures: `COCRDUPC.cbl:177-180,185-186`
/// and `203-204`, the inline literals of lines 745 and 789, and `WS-FILE-ERROR-MESSAGE` of lines
/// 133-152 hold exactly the bytes `COCRDSLC` declares, so the two programs share one definition
/// (COCRDUPC-010 to COCRDUPC-014, COCRDUPC-018, COCRDUPC-019). The card key and the answer of
/// the card read are shared the same way, because both programs read the card by its number
/// alone (COCRDUPC-016).
pub use super::cocrdslc::{
    ACCOUNT_FILTER_INVALID, CARD_FILTER_INVALID, CardKey, CardLookup, DID_NOT_FIND_ACCTCARD_COMBO,
    FILE_ERROR, NO_SEARCH_CRITERIA_RECEIVED, PROMPT_FOR_ACCT, PROMPT_FOR_CARD,
};

/// Width of `CRDNAMEI PIC X(50)` (`COCRDUP.CPY:72`) and `CCUP-NEW-CRDNAME PIC X(50)`
/// (`COCRDUPC.cbl:308`).
const NAME_WIDTH: usize = 50;

/// Width of `CRDSTCDI PIC X(1)` (`COCRDUP.CPY:78`) and `CCUP-NEW-CRDSTCD PIC X(1)`
/// (`COCRDUPC.cbl:313`).
const STATUS_WIDTH: usize = 1;

/// Width of `EXPMONI PIC X(2)` (`COCRDUP.CPY:84`) and `CCUP-NEW-EXPMON PIC X(2)`
/// (`COCRDUPC.cbl:311`).
const MONTH_WIDTH: usize = 2;

/// Width of `EXPYEARI PIC X(4)` (`COCRDUP.CPY:90`) and `CCUP-NEW-EXPYEAR PIC X(4)`
/// (`COCRDUPC.cbl:310`).
const YEAR_WIDTH: usize = 4;

/// Width of `CARDSIDI PIC X(16)` (`COCRDUP.CPY:66`) and `CC-CARD-NUM PIC X(16)` (`CVCRD01Y`).
const CARD_WIDTH: usize = 16;

/// `88 VALID-MONTH VALUES 1 THRU 12` on `CARD-MONTH-CHECK-N PIC 9(2)` (`COCRDUPC.cbl:92-95`,
/// COCRDUPC-030).
const VALID_MONTHS: RangeInclusive<u16> = 1..=12;

/// `88 VALID-YEAR VALUES 1950 THRU 2099` on `CARD-YEAR-CHECK-N PIC 9(4)` (`COCRDUPC.cbl:96-99`,
/// COCRDUPC-032).
const VALID_YEARS: RangeInclusive<u16> = 1950..=2099;

/// `FOUND-CARDS-FOR-ACCOUNT` of `WS-INFO-MSG PIC X(40)` (`COCRDUPC.cbl:160-161`): the info line
/// of state `S` (COCRDUPC-002, COCRDUPC-016, COCRDUPC-020, COCRDUPC-037).
pub const FOUND_CARDS_FOR_ACCOUNT: &str = "Details of selected card shown above";

/// `PROMPT-FOR-SEARCH-KEYS` (`COCRDUPC.cbl:162-163`): the info line while no card is fetched
/// (COCRDUPC-015, COCRDUPC-018).
pub const PROMPT_FOR_SEARCH_KEYS: &str = "Please enter Account and Card Number";

/// `PROMPT-FOR-CHANGES` (`COCRDUPC.cbl:164-165`): the info line of state `E` (COCRDUPC-021,
/// COCRDUPC-023).
pub const PROMPT_FOR_CHANGES: &str = "Update card details presented above.";

/// `PROMPT-FOR-CONFIRMATION` (`COCRDUPC.cbl:166-167`): the info line of state `N`
/// (COCRDUPC-034, COCRDUPC-035).
pub const PROMPT_FOR_CONFIRMATION: &str = "Changes validated.Press F5 to save";

/// `CONFIRM-UPDATE-SUCCESS` (`COCRDUPC.cbl:168-169`): the info line of state `C`
/// (COCRDUPC-039).
pub const CONFIRM_UPDATE_SUCCESS: &str = "Changes committed to database";

/// `INFORM-FAILURE` (`COCRDUPC.cbl:170-171`): the info line of states `L` and `F`
/// (COCRDUPC-036, COCRDUPC-040).
pub const INFORM_FAILURE: &str = "Changes unsuccessful. Please try again";

/// `WS-PROMPT-FOR-NAME` of `WS-RETURN-MSG PIC X(75)` (`COCRDUPC.cbl:181-182`): the name is
/// blank or all zeros (COCRDUPC-024).
pub const PROMPT_FOR_NAME: &str = "Card name not provided";

/// `WS-NAME-MUST-BE-ALPHA` (`COCRDUPC.cbl:183-184`): the name holds a byte other than a letter
/// or a space (COCRDUPC-025, COCRDUPC-026).
pub const NAME_MUST_BE_ALPHA: &str = "Card name can only contain alphabets and spaces";

/// `NO-CHANGES-DETECTED` (`COCRDUPC.cbl:187-188`): the values received equal the values fetched
/// once both are upper-cased (COCRDUPC-020 to COCRDUPC-022).
pub const NO_CHANGES_DETECTED: &str = "No change detected with respect to values fetched.";

/// `CARD-STATUS-MUST-BE-YES-NO` (`COCRDUPC.cbl:195-196`): the status is not `Y` or `N`
/// (COCRDUPC-027, COCRDUPC-028).
pub const CARD_STATUS_MUST_BE_YES_NO: &str = "Card Active Status must be Y or N";

/// `CARD-EXPIRY-MONTH-NOT-VALID` (`COCRDUPC.cbl:197-198`): the month is not `01` to `12`
/// (COCRDUPC-029, COCRDUPC-030).
pub const CARD_EXPIRY_MONTH_NOT_VALID: &str = "Card expiry month must be between 1 and 12";

/// `CARD-EXPIRY-YEAR-NOT-VALID` (`COCRDUPC.cbl:199-200`): the year is not `1950` to `2099`
/// (COCRDUPC-031, COCRDUPC-032).
pub const CARD_EXPIRY_YEAR_NOT_VALID: &str = "Invalid card expiry year";

/// `COULD-NOT-LOCK-FOR-UPDATE` (`COCRDUPC.cbl:205-206`): the read for update failed
/// (COCRDUPC-036).
pub const COULD_NOT_LOCK_FOR_UPDATE: &str = "Could not lock record for update";

/// `DATA-WAS-CHANGED-BEFORE-UPDATE` (`COCRDUPC.cbl:207-208`): the card read for update differs
/// from the snapshot (COCRDUPC-037).
pub const DATA_WAS_CHANGED_BEFORE_UPDATE: &str = "Record changed by some one else. Please review";

/// `LOCKED-BUT-UPDATE-FAILED` (`COCRDUPC.cbl:209-210`): the rewrite failed (COCRDUPC-040).
pub const LOCKED_BUT_UPDATE_FAILED: &str = "Update of record failed";

const _: () = assert!(FOUND_CARDS_FOR_ACCOUNT.len() <= 40);
const _: () = assert!(PROMPT_FOR_SEARCH_KEYS.len() <= 40);
const _: () = assert!(PROMPT_FOR_CHANGES.len() <= 40);
const _: () = assert!(PROMPT_FOR_CONFIRMATION.len() <= 40);
const _: () = assert!(CONFIRM_UPDATE_SUCCESS.len() <= 40);
const _: () = assert!(INFORM_FAILURE.len() <= 40);
const _: () = assert!(PROMPT_FOR_NAME.len() <= 75);
const _: () = assert!(NAME_MUST_BE_ALPHA.len() <= 75);
const _: () = assert!(NO_CHANGES_DETECTED.len() <= 75);
const _: () = assert!(CARD_STATUS_MUST_BE_YES_NO.len() <= 75);
const _: () = assert!(CARD_EXPIRY_MONTH_NOT_VALID.len() <= 75);
const _: () = assert!(CARD_EXPIRY_YEAR_NOT_VALID.len() <= 75);
const _: () = assert!(COULD_NOT_LOCK_FOR_UPDATE.len() <= 75);
const _: () = assert!(DATA_WAS_CHANGED_BEFORE_UPDATE.len() <= 75);
const _: () = assert!(LOCKED_BUT_UPDATE_FAILED.len() <= 75);

/// `CCUP-CHANGE-ACTION PIC X(1)` (`COCRDUPC.cbl:276-290`): the screen state a response leaves,
/// which the [`CardSnapshot`] carries and the client echoes (COCRDUPC-044).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardUpdateState {
    /// `CCUP-DETAILS-NOT-FETCHED` (spaces or low-values): the search screen.
    NotFetched,
    /// `CCUP-SHOW-DETAILS`, `S`: a card is shown and its details are enterable.
    ShowDetails,
    /// `CCUP-CHANGES-NOT-OK`, `E`: an edit failed.
    ChangesNotOk,
    /// `CCUP-CHANGES-OK-NOT-CONFIRMED`, `N`: the changes passed and await F5.
    ChangesOkNotConfirmed,
    /// `CCUP-CHANGES-OKAYED-AND-DONE`, `C`: the card was rewritten.
    ChangesOkayedAndDone,
    /// `CCUP-CHANGES-OKAYED-LOCK-ERROR`, `L`: the read for update failed.
    LockError,
    /// `CCUP-CHANGES-OKAYED-BUT-FAILED`, `F`: the rewrite failed.
    UpdateFailed,
}

impl CardUpdateState {
    /// The one-character code: a space for not fetched, otherwise `S`, `E`, `N`, `C`, `L` or
    /// `F` (COCRDUPC-044).
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::NotFetched => ' ',
            Self::ShowDetails => 'S',
            Self::ChangesNotOk => 'E',
            Self::ChangesOkNotConfirmed => 'N',
            Self::ChangesOkayedAndDone => 'C',
            Self::LockError => 'L',
            Self::UpdateFailed => 'F',
        }
    }

    /// The state an echoed code names: the empty text, a space or a low-value byte for not
    /// fetched, as `CCUP-DETAILS-NOT-FETCHED VALUES LOW-VALUES, SPACES` reads them, and each of
    /// the six letters for its state; `None` for every other text, which the program never sets
    /// and the request DTO refuses (`X-COCRDUPC-8`).
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "" | " " | "\0" => Some(Self::NotFetched),
            "S" => Some(Self::ShowDetails),
            "E" => Some(Self::ChangesNotOk),
            "N" => Some(Self::ChangesOkNotConfirmed),
            "C" => Some(Self::ChangesOkayedAndDone),
            "L" => Some(Self::LockError),
            "F" => Some(Self::UpdateFailed),
            _ => None,
        }
    }

    /// The info line `3250-SETUP-INFOMSG` derives from the state alone (`COCRDUPC.cbl:1138-1164`;
    /// COCRDUPC-015, COCRDUPC-016, COCRDUPC-021, COCRDUPC-034, COCRDUPC-036, COCRDUPC-039,
    /// COCRDUPC-040).
    #[must_use]
    pub const fn info_message(self) -> &'static str {
        match self {
            Self::NotFetched => PROMPT_FOR_SEARCH_KEYS,
            Self::ShowDetails => FOUND_CARDS_FOR_ACCOUNT,
            Self::ChangesNotOk => PROMPT_FOR_CHANGES,
            Self::ChangesOkNotConfirmed => PROMPT_FOR_CONFIRMATION,
            Self::ChangesOkayedAndDone => CONFIRM_UPDATE_SUCCESS,
            Self::LockError | Self::UpdateFailed => INFORM_FAILURE,
        }
    }

    /// Whether the `F5=Save F12=Cancel` labels are shown: only with the confirmation prompt of
    /// state `N` (`COCRDUPC.cbl:1315-1316`, COCRDUPC-034).
    #[must_use]
    pub const fn confirmation_keys_shown(self) -> bool {
        matches!(self, Self::ChangesOkNotConfirmed)
    }
}

/// The input a [`CardUpdateFailure`] names: the field the COBOL puts the cursor on, the first
/// failing one when several fail (COCRDUPC-023, `X-COCRDUPC-9`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardUpdateField {
    /// `ACCTSID`, the account of the endpoint 9 query.
    AcctId,
    /// `CARDSID`, the card number of the endpoint 9 query.
    CardNum,
    /// `CRDNAME`, the embossed name.
    EmbossedName,
    /// `CRDSTCD`, the active status.
    Status,
    /// `EXPMON`, the expiry month.
    ExpiryMonth,
    /// `EXPYEAR`, the expiry year.
    ExpiryYear,
}

impl CardUpdateField {
    /// The request field name `ScreenError.field` carries.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AcctId => "acct_id",
            Self::CardNum => "card_num",
            Self::EmbossedName => "embossed_name",
            Self::Status => "status",
            Self::ExpiryMonth => "expiry_month",
            Self::ExpiryYear => "expiry_year",
        }
    }
}

/// The status class of a [`CardUpdateFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// A key or detail edit failed, or nothing changed: 400 (COCRDUPC-010 to COCRDUPC-032).
    Input,
    /// No card has the key: 404 (COCRDUPC-003, COCRDUPC-018).
    NotFound,
    /// The card changed since it was fetched: 409 (COCRDUPC-037).
    Conflict,
    /// A read, a read for update or a rewrite failed in the store: 500 (COCRDUPC-019,
    /// COCRDUPC-036, COCRDUPC-040).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns: 400, 404, 409 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Conflict => 409,
            Self::Store => 500,
        }
    }
}

/// An answer of endpoint 9 or 10 that is an error: the program's message, the field it names,
/// the status class and the state the screen is left in (COCRDUPC-044). The message is a COBOL
/// literal and holds no input and no card number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CardUpdateFailure {
    message: &'static str,
    field: Option<CardUpdateField>,
    class: FailureClass,
    state: CardUpdateState,
}

impl CardUpdateFailure {
    /// A detail edit failure: 400 on `field`, state `E` (COCRDUPC-023).
    const fn edit(message: &'static str, field: CardUpdateField) -> Self {
        Self {
            message,
            field: Some(field),
            class: FailureClass::Input,
            state: CardUpdateState::ChangesNotOk,
        }
    }

    /// The no-change answer: 400 on the name, in the echoed `state` (COCRDUPC-020,
    /// COCRDUPC-021).
    const fn no_change(state: CardUpdateState) -> Self {
        Self {
            message: NO_CHANGES_DETECTED,
            field: Some(CardUpdateField::EmbossedName),
            class: FailureClass::Input,
            state,
        }
    }

    /// A search-key failure of [`cocrdslc::edit`], whose texts are COCRDUPC's own: 400 on its
    /// field, state not fetched (COCRDUPC-010 to COCRDUPC-015).
    fn search(failure: DetailFailure) -> Self {
        Self {
            message: failure.message(),
            field: Some(field_of(failure.field())),
            class: class_of(failure.class()),
            state: CardUpdateState::NotFetched,
        }
    }

    /// The card read failed with `message` and `class`; the account is marked (COCRDUPC-003,
    /// COCRDUPC-018, COCRDUPC-019).
    const fn read(message: &'static str, class: FailureClass, state: CardUpdateState) -> Self {
        Self {
            message,
            field: Some(CardUpdateField::AcctId),
            class,
            state,
        }
    }

    /// The 409 of `9300-CHECK-CHANGE-IN-REC`: nothing written, state `S`; the screen takes the
    /// card's current values from endpoint 9 (COCRDUPC-037).
    const fn changed_by_another_user() -> Self {
        Self {
            message: DATA_WAS_CHANGED_BEFORE_UPDATE,
            field: None,
            class: FailureClass::Conflict,
            state: CardUpdateState::ShowDetails,
        }
    }

    /// The read for update of a save failed, or no card has the `card_ref`: 500 `Could not lock
    /// record for update`, nothing written, state `L` (COCRDUPC-036).
    #[must_use]
    pub const fn lock_failed() -> Self {
        Self {
            message: COULD_NOT_LOCK_FOR_UPDATE,
            field: None,
            class: FailureClass::Store,
            state: CardUpdateState::LockError,
        }
    }

    /// The rewrite of a save failed: 500 `Update of record failed`, the card keeps its stored
    /// values, state `F` (COCRDUPC-040). Also the answer when the rewritten record cannot be
    /// built, which is no parity result (COCRDUPC-038, `X-COCRDUPC-3`).
    #[must_use]
    pub const fn rewrite_failed() -> Self {
        Self {
            message: LOCKED_BUT_UPDATE_FAILED,
            field: None,
            class: FailureClass::Store,
            state: CardUpdateState::UpdateFailed,
        }
    }

    /// The message, as the response's `ScreenError` carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the message names, if any.
    #[must_use]
    pub const fn field(&self) -> Option<CardUpdateField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }

    /// The state the screen is left in, whose info line it shows (COCRDUPC-044).
    #[must_use]
    pub const fn state(&self) -> CardUpdateState {
        self.state
    }
}

impl fmt::Display for CardUpdateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for CardUpdateFailure {}

/// The field of a [`cocrdslc::edit`] failure.
const fn field_of(field: DetailField) -> CardUpdateField {
    match field {
        DetailField::AcctId => CardUpdateField::AcctId,
        DetailField::CardNum => CardUpdateField::CardNum,
    }
}

/// The status class of a [`cocrdslc::edit`] failure.
const fn class_of(class: cocrdslc::FailureClass) -> FailureClass {
    match class {
        cocrdslc::FailureClass::Input => FailureClass::Input,
        cocrdslc::FailureClass::NotFound => FailureClass::NotFound,
        cocrdslc::FailureClass::Store => FailureClass::Store,
    }
}

/// One map field after `1100-RECEIVE-MAP` (`COCRDUPC.cbl:586-635`, COCRDUPC-009).
#[derive(Clone, Copy)]
enum Received<const N: usize> {
    /// The field's `N` bytes: the text left-justified and space-padded, as BMS delivers an
    /// input field, or `N` low-values when it is all spaces or `*` followed only by spaces.
    Field([u8; N]),
    /// Text longer than the field, which no terminal can send: it fails its field's edit and
    /// equals no fetched value, and is never cut into a shorter value.
    Overlong,
}

impl<const N: usize> Received<N> {
    /// `text` as the program holds it after `1100-RECEIVE-MAP`.
    fn from_text(text: &str) -> Self {
        let bytes = text.as_bytes();
        if bytes.len() > N {
            return Self::Overlong;
        }
        let field = *PicX::<N>::moved_from(bytes).as_bytes();
        let asterisk_alone = matches!(
            field.split_first(),
            Some((b'*', rest)) if rest.iter().all(|&byte| byte == b' ')
        );
        if asterisk_alone || field.iter().all(|&byte| byte == b' ') {
            Self::Field([0; N])
        } else {
            Self::Field(field)
        }
    }

    /// The blank test of every edit: `EQUAL LOW-VALUES OR EQUAL SPACES OR EQUAL ZEROS`
    /// (`COCRDUPC.cbl:768-770,811-813`).
    fn is_blank(self) -> bool {
        match self {
            Self::Field(bytes) => [0, b' ', b'0']
                .iter()
                .any(|&fill| bytes.iter().all(|&byte| byte == fill)),
            Self::Overlong => false,
        }
    }

    /// Whether the field equals `old` once both are upper-cased, the comparison of
    /// `FUNCTION UPPER-CASE` (`COCRDUPC.cbl:680-681`, COCRDUPC-020, COCRDUPC-022).
    fn same_as(self, old: [u8; N]) -> bool {
        match self {
            Self::Field(bytes) => upper(bytes) == upper(old),
            Self::Overlong => false,
        }
    }
}

/// `a` to `z` converted to `A` to `Z`, every other byte unchanged: `INSPECT ... CONVERTING
/// LIT-LOWER TO LIT-UPPER` and `FUNCTION UPPER-CASE` (`COCRDUPC.cbl:1356-1358`, COCRDUPC-017).
fn upper<const N: usize>(bytes: [u8; N]) -> [u8; N] {
    bytes.map(|byte| byte.to_ascii_uppercase())
}

/// The value of `bytes` when every byte is an ASCII digit; four digits at most reach here.
fn digits_value(bytes: &[u8]) -> Option<u16> {
    bytes.iter().try_fold(0_u16, |value, &byte| {
        let digit = byte.checked_sub(b'0').filter(|&digit| digit <= 9)?;
        value.checked_mul(10)?.checked_add(u16::from(digit))
    })
}

/// The search keys of endpoint 9 (`CardLookupQuery { acct_id, card_num? | card_ref? }`),
/// edited as COCRDUPC edits them.
///
/// - The list path, a `card_ref` from a COCRDLI row with no card number typed: no key edit
///   runs and both keys are taken as valid, the account as the 11 digits the row carries, all
///   zeros included (COCRDUPC-002). An account that is not 11 digits cannot come from a list
///   row and is edited as on the search path.
/// - The search path: `1210-EDIT-ACCOUNT`, `1220-EDIT-CARD` and the cross-field edit of
///   `1200-EDIT-MAP-INPUTS`, whose texts and order are those of [`cocrdslc::edit`]: blank means
///   not supplied, all spaces, `*` alone, low-values or all zeros; the first message stands,
///   except that `No input received` replaces the account prompt when neither key is supplied
///   (COCRDUPC-010 to COCRDUPC-014).
///
/// # Errors
///
/// A [`CardUpdateFailure`] of class [`FailureClass::Input`] (400) in state not fetched, so the
/// search screen stays as it was (COCRDUPC-015): [`NO_SEARCH_CRITERIA_RECEIVED`] or
/// [`PROMPT_FOR_ACCT`] on the account (COCRDUPC-010, COCRDUPC-014), [`ACCOUNT_FILTER_INVALID`]
/// on the account (COCRDUPC-011), [`PROMPT_FOR_CARD`] on the card (COCRDUPC-012),
/// [`CARD_FILTER_INVALID`] on the card (COCRDUPC-013), the last also for a card number typed
/// beside a handle. `HALTED(X-COCRDUPC-H2)`: an account or card of zero-nibble bytes takes the
/// class-test message, which is no parity result.
pub fn edit_keys(
    acct: &str,
    card_num: Option<&str>,
    card_ref: Option<CardRef>,
) -> Result<(PicU<11>, CardKey), CardUpdateFailure> {
    let no_card_typed =
        card_num.is_none_or(|text| Received::<CARD_WIDTH>::from_text(text).is_blank());
    let list_entry = card_ref
        .filter(|_| no_card_typed)
        .zip(PicU::<11>::decode(acct.as_bytes()).ok());
    if let Some((handle, acct_id)) = list_entry {
        return Ok((acct_id, CardKey::ByRef(handle)));
    }
    cocrdslc::edit(acct, card_num, card_ref).map_err(CardUpdateFailure::search)
}

/// The fetched expiry date a [`CardSnapshot`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldDate {
    /// `CCUP-OLD-EXPYEAR`, `-EXPMON` and `-EXPDAY` as one typed date, blank for a blank stored
    /// date.
    Known(PicDate),
    /// An echoed date four year digits cannot carry: it equals no stored date.
    Unknown,
}

/// The values `CCUP-OLD-DETAILS` holds after `9000-READ-DATA`, and the screen state
/// (`COCRDUPC.cbl:291-301,1343-1369`; COCRDUPC-044).
///
/// It holds the account as entered, the name upper-cased, the typed expiry date with its day
/// and the status, and no card number and no CVV: the card is identified by the `card_ref` of
/// the path (COCRDUPC-043, `E32`). The client echoes it unchanged; it grants nothing
/// (`X-COCRDUPC-1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSnapshot {
    state: CardUpdateState,
    acct_id: Option<PicU<11>>,
    embossed_name: PicX<NAME_WIDTH>,
    expiry: HeldDate,
    status: PicX<STATUS_WIDTH>,
}

/// A snapshot as the client echoes it, the input of [`CardSnapshot::from_fields`]: the state,
/// the account as 11 digits, the name and status as text and the typed expiry date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardSnapshotFields<'a> {
    /// `CCUP-CHANGE-ACTION`, decoded by [`CardUpdateState::from_code`].
    pub state: CardUpdateState,
    /// `CCUP-OLD-ACCTID`.
    pub acct_id: &'a str,
    /// `CCUP-OLD-CRDNAME`.
    pub embossed_name: &'a str,
    /// `CCUP-OLD-EXPIRAION-DATE`; `None` for a blank stored date.
    pub expiration_date: Option<NaiveDate>,
    /// `CCUP-OLD-CRDSTCD`.
    pub status: &'a str,
}

impl CardSnapshot {
    /// `9000-READ-DATA` (COCRDUPC-017, COCRDUPC-044): the account `acct_id` and the card
    /// `record`, the name upper-cased, in state `S`.
    fn fetched(acct_id: PicU<11>, record: &CardRecord) -> Self {
        Self {
            state: CardUpdateState::ShowDetails,
            acct_id: Some(acct_id),
            embossed_name: PicX::from_array(upper(*record.embossed_name.as_bytes())),
            expiry: HeldDate::Known(record.expiraion_date),
            status: record.active_status.clone(),
        }
    }

    /// The snapshot a client echoed (COCRDUPC-044). Total: every text yields a snapshot.
    ///
    /// The name and status are read as a `MOVE` into their fields, their first bytes kept and
    /// the rest spaces. An account that is not exactly 11 ASCII digits, or a date whose year four
    /// digits cannot carry, is held as unknown: it equals no stored value, so it reads as a
    /// change and, at a save, as a record changed by another user or a failed rewrite.
    #[must_use]
    pub fn from_fields(fields: &CardSnapshotFields<'_>) -> Self {
        Self {
            state: fields.state,
            acct_id: PicU::decode(fields.acct_id.as_bytes()).ok(),
            embossed_name: PicX::moved_from(fields.embossed_name.as_bytes()),
            expiry: PicDate::new(fields.expiration_date).map_or(HeldDate::Unknown, HeldDate::Known),
            status: PicX::moved_from(fields.status.as_bytes()),
        }
    }

    /// The state the snapshot was returned with.
    #[must_use]
    pub const fn state(&self) -> CardUpdateState {
        self.state
    }

    /// `CCUP-OLD-ACCTID`, the account the save writes into the card (COCRDUPC-038); `None` when
    /// the echoed text was not 11 digits.
    #[must_use]
    pub const fn acct_id(&self) -> Option<PicU<11>> {
        self.acct_id
    }

    /// `CCUP-OLD-CRDNAME`, upper-cased at the fetch (COCRDUPC-017).
    #[must_use]
    pub const fn embossed_name(&self) -> &PicX<NAME_WIDTH> {
        &self.embossed_name
    }

    /// The fetched expiry date; `None` when it is blank or the echoed date was unknown.
    #[must_use]
    pub const fn expiration_date(&self) -> Option<NaiveDate> {
        match self.expiry {
            HeldDate::Known(date) => date.value(),
            HeldDate::Unknown => None,
        }
    }

    /// `CCUP-OLD-CRDSTCD`.
    #[must_use]
    pub const fn status(&self) -> &PicX<STATUS_WIDTH> {
        &self.status
    }

    /// The same values in the state a response leaves the screen in, as the client echoes them
    /// next (COCRDUPC-044).
    #[must_use]
    pub fn with_state(mut self, state: CardUpdateState) -> Self {
        self.state = state;
        self
    }

    /// `CCUP-OLD-EXPYEAR` and `CCUP-OLD-EXPMON` as text, the date's positions 1-4 and 6-7
    /// (`COCRDUPC.cbl:1361-1364`); spaces for a blank date, `None` for an unknown one.
    fn year_and_month(&self) -> Option<([u8; YEAR_WIDTH], [u8; MONTH_WIDTH])> {
        let HeldDate::Known(date) = self.expiry else {
            return None;
        };
        let text = date_text(date);
        let [year @ .., _, _, _, _, _, _] = text;
        let [_, _, _, _, _, month @ .., _, _, _] = text;
        Some((year, month))
    }

    /// `CCUP-OLD-EXPDAY`, the day the save keeps (COCRDUPC-038); `None` when the stored date is
    /// blank or unknown, whose two spaces make no calendar date (`X-COCRDUPC-3`).
    fn expiry_day(&self) -> Option<u32> {
        match self.expiry {
            HeldDate::Known(date) => date.value().map(|day| day.day()),
            HeldDate::Unknown => None,
        }
    }
}

/// `date` as its ten bytes, `YYYY-MM-DD` or ten spaces (PIC-015).
fn date_text(date: PicDate) -> [u8; PicDate::WIDTH] {
    let mut text = Vec::with_capacity(PicDate::WIDTH);
    date.encode_into(&mut text);
    // `encode_into` always writes exactly `PicDate::WIDTH` bytes, so the fallback of ten spaces
    // is never taken; it only keeps this function free of a panic path.
    <[u8; PicDate::WIDTH]>::try_from(text).unwrap_or([b' '; PicDate::WIDTH])
}

/// The response of endpoint 9 for a found card: what `3200-SETUP-SCREEN-VARS` shows in state
/// `S` (`COCRDUPC.cbl:1082-1134`; COCRDUPC-002, COCRDUPC-016, COCRDUPC-017, COCRDUPC-042).
///
/// It carries no CVV, and the card number only as [`CardNumber`], whose `Debug` and `Serialize`
/// show twelve `*` and the last four digits (`E32`). No error message accompanies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardUpdateView {
    /// The card's handle, which endpoint 10 takes in its path (COCRDUPC-043).
    pub card_ref: CardRef,
    /// `CARDSIDO`, redacted on egress (COCRDUPC-042).
    pub card_num: CardNumber,
    /// `ACCTSIDO`: the account as entered, or the list row's, never the card's own
    /// `CARD-ACCT-ID` (COCRDUPC-016, `X-COCRDUPC-2`).
    pub acct_id: PicU<11>,
    /// `CRDNAMEO`: the stored name with `a` to `z` upper-cased (COCRDUPC-017).
    pub embossed_name: PicX<NAME_WIDTH>,
    /// `CARD-EXPIRAION-DATE`, typed; `None` for a blank stored date. The screen shows its month
    /// and year and keeps the day out of sight (COCRDUPC-017, CVACT02Y-005).
    pub expiration_date: Option<NaiveDate>,
    /// `CRDSTCDO`, the stored status.
    pub status: PicX<STATUS_WIDTH>,
    /// The fetched values in state `S`, which the client echoes to endpoint 10 (COCRDUPC-044).
    pub snapshot: CardSnapshot,
    /// `INFOMSGO`: always [`FOUND_CARDS_FOR_ACCOUNT`].
    pub info_message: &'static str,
}

/// `9000-READ-DATA` for endpoint 9: the response for the outcome of the card read by `key`,
/// which [`edit_keys`] returned (COCRDUPC-002, COCRDUPC-003, COCRDUPC-016 to COCRDUPC-019,
/// COCRDUPC-042, COCRDUPC-044).
///
/// A found card gives a [`CardUpdateView`] with the account of the request, the card's handle,
/// its redacted number, its upper-cased name, typed expiry date and status, the snapshot of
/// those values in state `S`, and [`FOUND_CARDS_FOR_ACCOUNT`].
///
/// # Errors
///
/// A [`CardUpdateFailure`] on the account, whose state is `S` on the list path
/// ([`CardKey::ByRef`]), where the COBOL sets it before the read, and not fetched on the search
/// path ([`CardKey::ByNumber`]):
///
/// - [`CardLookup::NotFound`]: [`DID_NOT_FIND_ACCTCARD_COMBO`], class
///   [`FailureClass::NotFound`] (404; COCRDUPC-003, COCRDUPC-018);
/// - [`CardLookup::Error`]: [`FILE_ERROR`], class [`FailureClass::Store`] (500), whose `RESP`
///   and `RESP2` fields are `HALTED(H4)` (COCRDUPC-019, COCRDUPC-047).
pub fn fetch(key: &CardKey, lookup: CardLookup<'_>) -> Result<CardUpdateView, CardUpdateFailure> {
    let failed_state = match key {
        CardKey::ByRef(_) => CardUpdateState::ShowDetails,
        CardKey::ByNumber(_) => CardUpdateState::NotFetched,
    };
    match lookup {
        CardLookup::Found {
            acct_id,
            card_ref,
            record,
        } => {
            let snapshot = CardSnapshot::fetched(acct_id, record);
            Ok(CardUpdateView {
                card_ref,
                card_num: record.num.clone(),
                acct_id,
                embossed_name: snapshot.embossed_name.clone(),
                expiration_date: record.expiraion_date.value(),
                status: record.active_status.clone(),
                snapshot,
                info_message: FOUND_CARDS_FOR_ACCOUNT,
            })
        }
        CardLookup::NotFound => Err(CardUpdateFailure::read(
            DID_NOT_FIND_ACCTCARD_COMBO,
            FailureClass::NotFound,
            failed_state,
        )),
        CardLookup::Error => Err(CardUpdateFailure::read(
            FILE_ERROR,
            FailureClass::Store,
            failed_state,
        )),
    }
}

/// The key of endpoint 10 (`CardUpdateRequest.aid`). The screen submits every other key as
/// ENTER and keeps F3 and F12 to itself (COCRDUPC-004, COCRDUPC-006, COCRDUPC-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aid {
    /// ENTER: the change check and the edits, then the confirmation prompt.
    Enter,
    /// F5: the save, valid only in state `N`; in any other state it is ENTER (COCRDUPC-007).
    Pf5,
}

/// The detail fields of an endpoint 10 request, each raw screen text: the edits own their
/// messages (Typed Dates and Timestamps; COCRDUPC-009). The expiry day is never input: it is
/// the snapshot's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CardUpdateInput<'a> {
    /// `CRDNAMEI`, up to 50 characters.
    pub embossed_name: &'a str,
    /// `CRDSTCDI`, one character.
    pub status: &'a str,
    /// `EXPMONI`, two digits; the screen fills a shorter entry with `0` on the left
    /// (COCRDUPC-033).
    pub expiry_month: &'a str,
    /// `EXPYEARI`, four digits, filled the same way.
    pub expiry_year: &'a str,
}

/// The four detail fields after `1100-RECEIVE-MAP`.
#[derive(Clone, Copy)]
struct ReceivedFields {
    name: Received<NAME_WIDTH>,
    status: Received<STATUS_WIDTH>,
    month: Received<MONTH_WIDTH>,
    year: Received<YEAR_WIDTH>,
}

impl ReceivedFields {
    fn new(input: &CardUpdateInput<'_>) -> Self {
        Self {
            name: Received::from_text(input.embossed_name),
            status: Received::from_text(input.status),
            month: Received::from_text(input.expiry_month),
            year: Received::from_text(input.expiry_year),
        }
    }

    /// The change check of `1200-EDIT-MAP-INPUTS`: whether `UPPER-CASE(CCUP-NEW-CARDDATA)`
    /// equals `UPPER-CASE(CCUP-OLD-CARDDATA)` (`COCRDUPC.cbl:680-683`; COCRDUPC-020 to
    /// COCRDUPC-022). The received day is the snapshot's, so the name, year, month and status
    /// decide it.
    fn unchanged(self, old: &CardSnapshot) -> bool {
        old.year_and_month().is_some_and(|(year, month)| {
            self.name.same_as(*old.embossed_name.as_bytes())
                && self.year.same_as(year)
                && self.month.same_as(month)
                && self.status.same_as(*old.status.as_bytes())
        })
    }
}

/// The detail values once `1230-EDIT-NAME` to `1260-EDIT-EXPIRY-YEAR` pass (COCRDUPC-023 to
/// COCRDUPC-032): what a save writes (COCRDUPC-038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardChanges {
    embossed_name: PicX<NAME_WIDTH>,
    status: PicX<STATUS_WIDTH>,
    expiry_month: u16,
    expiry_year: u16,
}

impl CardChanges {
    /// `CCUP-NEW-CRDNAME` exactly as received: letters and spaces, letter case kept.
    #[must_use]
    pub const fn embossed_name(&self) -> &PicX<NAME_WIDTH> {
        &self.embossed_name
    }

    /// `CCUP-NEW-CRDSTCD`: `Y` or `N`.
    #[must_use]
    pub const fn status(&self) -> &PicX<STATUS_WIDTH> {
        &self.status
    }

    /// `CCUP-NEW-EXPMON`: 1 to 12.
    #[must_use]
    pub fn expiry_month(&self) -> u32 {
        u32::from(self.expiry_month)
    }

    /// `CCUP-NEW-EXPYEAR`: 1950 to 2099.
    #[must_use]
    pub fn expiry_year(&self) -> i32 {
        i32::from(self.expiry_year)
    }
}

/// `1230-EDIT-NAME` (`COCRDUPC.cbl:806-843`; COCRDUPC-024 to COCRDUPC-026).
fn edit_name(field: Received<NAME_WIDTH>) -> Result<PicX<NAME_WIDTH>, CardUpdateFailure> {
    let Received::Field(bytes) = field else {
        return Err(CardUpdateFailure::edit(
            NAME_MUST_BE_ALPHA,
            CardUpdateField::EmbossedName,
        ));
    };
    if field.is_blank() {
        return Err(CardUpdateFailure::edit(
            PROMPT_FOR_NAME,
            CardUpdateField::EmbossedName,
        ));
    }
    // Converting every letter to a space must leave only spaces, whose `FUNCTION TRIM` has
    // length 0 (COCRDUPC-025).
    if bytes
        .iter()
        .all(|&byte| byte == b' ' || byte.is_ascii_alphabetic())
    {
        Ok(PicX::from_array(bytes))
    } else {
        Err(CardUpdateFailure::edit(
            NAME_MUST_BE_ALPHA,
            CardUpdateField::EmbossedName,
        ))
    }
}

/// `1240-EDIT-CARDSTATUS` (`COCRDUPC.cbl:845-876`; COCRDUPC-027, COCRDUPC-028): only an
/// upper-case `Y` or `N` passes; blank, `0` and every other byte share the message.
fn edit_status(field: Received<STATUS_WIDTH>) -> Result<PicX<STATUS_WIDTH>, CardUpdateFailure> {
    match field {
        Received::Field(bytes @ [b'Y' | b'N']) => Ok(PicX::from_array(bytes)),
        Received::Field(_) | Received::Overlong => Err(CardUpdateFailure::edit(
            CARD_STATUS_MUST_BE_YES_NO,
            CardUpdateField::Status,
        )),
    }
}

/// `1250-EDIT-EXPIRY-MON` (`COCRDUPC.cbl:877-912`; COCRDUPC-029, COCRDUPC-030): two digits from
/// `01` to `12`. Blank and `00` fail with the same message as `13` to `99`.
/// `HALTED(X-COCRDUPC-H2)`: a month holding a non-digit fails too, which is no parity result.
fn edit_month(field: Received<MONTH_WIDTH>) -> Result<u16, CardUpdateFailure> {
    let month = match field {
        Received::Field(bytes) => digits_value(&bytes).filter(|month| VALID_MONTHS.contains(month)),
        Received::Overlong => None,
    };
    month.ok_or(CardUpdateFailure::edit(
        CARD_EXPIRY_MONTH_NOT_VALID,
        CardUpdateField::ExpiryMonth,
    ))
}

/// `1260-EDIT-EXPIRY-YEAR` (`COCRDUPC.cbl:913-947`; COCRDUPC-031, COCRDUPC-032): four digits
/// from `1950` to `2099`. Blank and `0000` fail with the same message as every other year.
/// `HALTED(X-COCRDUPC-H2)`: a year holding a non-digit fails too, which is no parity result.
fn edit_year(field: Received<YEAR_WIDTH>) -> Result<u16, CardUpdateFailure> {
    let year = match field {
        Received::Field(bytes) => digits_value(&bytes).filter(|year| VALID_YEARS.contains(year)),
        Received::Overlong => None,
    };
    year.ok_or(CardUpdateFailure::edit(
        CARD_EXPIRY_YEAR_NOT_VALID,
        CardUpdateField::ExpiryYear,
    ))
}

/// The four edits in program order. The COBOL runs all four and keeps the first message, and
/// the response names only that edit's field (`X-COCRDUPC-9`), so the first failure decides.
fn edit_received(received: ReceivedFields) -> Result<CardChanges, CardUpdateFailure> {
    Ok(CardChanges {
        embossed_name: edit_name(received.name)?,
        status: edit_status(received.status)?,
        expiry_month: edit_month(received.month)?,
        expiry_year: edit_year(received.year)?,
    })
}

/// The detail edits of endpoint 10 alone, `1230-EDIT-NAME`, `1240-EDIT-CARDSTATUS`,
/// `1250-EDIT-EXPIRY-MON` and `1260-EDIT-EXPIRY-YEAR` over the fields as `1100-RECEIVE-MAP`
/// holds them (COCRDUPC-009, COCRDUPC-023 to COCRDUPC-032). [`decide`] runs them after the
/// change check, as `1200-EDIT-MAP-INPUTS` does.
///
/// Each text is read as BMS delivers its field: left-justified and space-padded, and taken as
/// low-values when it is all spaces or `*` followed only by spaces. A text longer than its field
/// fails that field's edit and is never cut.
///
/// # Errors
///
/// The first failing edit, class [`FailureClass::Input`] (400), state `E`, in this order:
///
/// - name blank or all zeros: [`PROMPT_FOR_NAME`]; a byte other than a letter or a space, or
///   more than 50 bytes: [`NAME_MUST_BE_ALPHA`] (COCRDUPC-024 to COCRDUPC-026);
/// - status other than `Y` or `N`: [`CARD_STATUS_MUST_BE_YES_NO`] (COCRDUPC-027, COCRDUPC-028);
/// - month other than `01` to `12`: [`CARD_EXPIRY_MONTH_NOT_VALID`] (COCRDUPC-029,
///   COCRDUPC-030);
/// - year other than `1950` to `2099`: [`CARD_EXPIRY_YEAR_NOT_VALID`] (COCRDUPC-031,
///   COCRDUPC-032).
pub fn edit(input: &CardUpdateInput<'_>) -> Result<CardChanges, CardUpdateFailure> {
    edit_received(ReceivedFields::new(input))
}

/// A 200 answer of endpoint 10: the screen `3000-SEND-MAP` sends after `2000-DECIDE-ACTION`
/// (COCRDUPC-034, COCRDUPC-035, COCRDUPC-039).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardUpdateOutcome {
    /// State `N`, info [`PROMPT_FOR_CONFIRMATION`], no error message, the `F5=Save F12=Cancel`
    /// labels shown and every field read-only; nothing written. The fields show the values
    /// received (COCRDUPC-034, COCRDUPC-035).
    Validated {
        /// The fetched values in state `N`.
        snapshot: CardSnapshot,
    },
    /// State `C`, info [`CONFIRM_UPDATE_SUCCESS`], every field read-only: the card was
    /// rewritten (COCRDUPC-039).
    Committed {
        /// The fetched values in state `C`.
        snapshot: CardSnapshot,
    },
    /// The empty search screen: state not fetched, info [`PROMPT_FOR_SEARCH_KEYS`].
    /// `0000-MAIN` answers so for any key in states `C`, `L` and `F` (COCRDUPC-005); the
    /// replacement also answers so for an echoed state not fetched, which the screen never sends
    /// to endpoint 10 because it searches through endpoint 9. Neither request reaches endpoint
    /// 10 from the screen, so this is no parity result.
    Restart,
}

impl CardUpdateOutcome {
    /// The state the screen is left in.
    #[must_use]
    pub const fn state(&self) -> CardUpdateState {
        match self {
            Self::Validated { .. } => CardUpdateState::ChangesOkNotConfirmed,
            Self::Committed { .. } => CardUpdateState::ChangesOkayedAndDone,
            Self::Restart => CardUpdateState::NotFetched,
        }
    }

    /// The info line of that state.
    #[must_use]
    pub const fn info_message(&self) -> &'static str {
        self.state().info_message()
    }

    /// Whether the `F5=Save F12=Cancel` labels are shown (COCRDUPC-034).
    #[must_use]
    pub const fn confirmation_keys_shown(&self) -> bool {
        self.state().confirmation_keys_shown()
    }

    /// The snapshot the client echoes next; `None` after [`CardUpdateOutcome::Restart`], when
    /// the screen searches again.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&CardSnapshot> {
        match self {
            Self::Validated { snapshot } | Self::Committed { snapshot } => Some(snapshot),
            Self::Restart => None,
        }
    }
}

/// What [`decide`] asks of the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Answer with this outcome; nothing is read or written.
    Respond(CardUpdateOutcome),
    /// Run the save of `9200-WRITE-PROCESSING` that the effects describe (COCRDUPC-036 to
    /// COCRDUPC-040).
    Save(CardSaveEffects),
}

/// `1200-EDIT-MAP-INPUTS` and `2000-DECIDE-ACTION` for endpoint 10, with the state of
/// `snapshot` standing for `CCUP-CHANGE-ACTION` (`COCRDUPC.cbl:641-719,948-1031`; COCRDUPC-007,
/// COCRDUPC-020 to COCRDUPC-035, COCRDUPC-041).
///
/// The change check runs first: the name, status, month and year received against the
/// snapshot's, both sides upper-cased; only a change runs the edits. How each COBOL state maps
/// to the request:
///
/// - `S` or `E`, with ENTER, or with F5, which `0000-MAIN` turns into ENTER outside state `N`
///   (COCRDUPC-007): no change is [`NO_CHANGES_DETECTED`] in the echoed state (COCRDUPC-020,
///   COCRDUPC-021); a change runs the four edits, whose first failure leaves state `E`
///   (COCRDUPC-023) and whose success is [`CardUpdateOutcome::Validated`] (COCRDUPC-034).
/// - `N` with ENTER: no edit runs; a change is [`CardUpdateOutcome::Validated`] again
///   (COCRDUPC-035), and no change, which a 3270 cannot send in state `N`, is the no-change
///   answer in state `N`, as the COBOL gives it.
/// - `N` with F5: the COBOL saves, receiving back the values it validated. A REST client can
///   send anything, so the change check and the edits run again: a change that passes is
///   [`Decision::Save`]; otherwise the answer ENTER gets in state `S` (COCRDUPC-041,
///   `X-COCRDUPC-1`).
/// - `C`, `L`, `F` and not fetched: [`CardUpdateOutcome::Restart`] (COCRDUPC-005).
///
/// # Errors
///
/// The no-change 400 on the name, in the echoed state or, for F5 in state `N`, in state `S`;
/// or the first failing edit's 400 in state `E`, as [`edit`] lists them.
pub fn decide(
    aid: Aid,
    snapshot: &CardSnapshot,
    input: &CardUpdateInput<'_>,
) -> Result<Decision, CardUpdateFailure> {
    let received = ReceivedFields::new(input);
    let unchanged = received.unchanged(snapshot);
    match snapshot.state() {
        CardUpdateState::NotFetched
        | CardUpdateState::ChangesOkayedAndDone
        | CardUpdateState::LockError
        | CardUpdateState::UpdateFailed => Ok(Decision::Respond(CardUpdateOutcome::Restart)),
        CardUpdateState::ChangesOkNotConfirmed => match (aid, unchanged) {
            (Aid::Pf5, false) => {
                let changes = edit_received(received)?;
                Ok(Decision::Save(CardSaveEffects {
                    snapshot: snapshot.clone(),
                    changes,
                }))
            }
            (Aid::Pf5, true) => Err(CardUpdateFailure::no_change(CardUpdateState::ShowDetails)),
            (Aid::Enter, false) => Ok(validated(snapshot)),
            (Aid::Enter, true) => Err(CardUpdateFailure::no_change(
                CardUpdateState::ChangesOkNotConfirmed,
            )),
        },
        echoed @ (CardUpdateState::ShowDetails | CardUpdateState::ChangesNotOk) => {
            if unchanged {
                Err(CardUpdateFailure::no_change(echoed))
            } else {
                edit_received(received).map(|_| validated(snapshot))
            }
        }
    }
}

/// [`CardUpdateOutcome::Validated`] over `snapshot`'s values.
fn validated(snapshot: &CardSnapshot) -> Decision {
    Decision::Respond(CardUpdateOutcome::Validated {
        snapshot: snapshot
            .clone()
            .with_state(CardUpdateState::ChangesOkNotConfirmed),
    })
}

/// `9300-CHECK-CHANGE-IN-REC` (`COCRDUPC.cbl:1498-1523`; COCRDUPC-037): whether `current`, the
/// card read for update, still holds the values `snapshot` fetched.
///
/// The card's name is upper-cased and compared with the snapshot's, then its typed expiry date
/// (the year, month and day of the COBOL's three comparisons) and its status. A change by
/// another user to the letter case of the name alone is not detected. The CVV term is dropped,
/// because no snapshot carries a CVV (`X-COCRDUPC-4`).
///
/// # Errors
///
/// [`DATA_WAS_CHANGED_BEFORE_UPDATE`], class [`FailureClass::Conflict`] (409), state `S`, when
/// any compared value differs: nothing is written, and the screen takes the card's current
/// values from endpoint 9 (COCRDUPC-037).
pub fn check_change_in_rec(
    snapshot: &CardSnapshot,
    current: &CardRecord,
) -> Result<(), CardUpdateFailure> {
    let unchanged = upper(*current.embossed_name.as_bytes()) == *snapshot.embossed_name.as_bytes()
        && snapshot.expiry == HeldDate::Known(current.expiraion_date)
        && current.active_status == snapshot.status;
    if unchanged {
        Ok(())
    } else {
        Err(CardUpdateFailure::changed_by_another_user())
    }
}

/// The save F5 confirmed in state `N`: the snapshot and the changes the rewrite of
/// `9200-WRITE-PROCESSING` takes (COCRDUPC-036 to COCRDUPC-040).
///
/// The service reads the card that `card_ref` names for update ([`CardUpdateFailure::lock_failed`]
/// when it cannot or no card has the handle), runs [`CardSaveEffects::check_change_in_rec`] on
/// the record it holds, rewrites the card with [`CardSaveEffects::rewritten`] and answers
/// [`CardSaveEffects::committed`], or [`CardUpdateFailure::rewrite_failed`] when the rewrite
/// fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSaveEffects {
    snapshot: CardSnapshot,
    changes: CardChanges,
}

impl CardSaveEffects {
    /// The snapshot the save compares and whose account it writes.
    #[must_use]
    pub const fn snapshot(&self) -> &CardSnapshot {
        &self.snapshot
    }

    /// The validated changes the save writes.
    #[must_use]
    pub const fn changes(&self) -> &CardChanges {
        &self.changes
    }

    /// [`check_change_in_rec`] of this save's snapshot against `held`, the card read for
    /// update (COCRDUPC-037).
    ///
    /// # Errors
    ///
    /// As [`check_change_in_rec`]: the 409 in state `S`.
    pub fn check_change_in_rec(&self, held: &CardRecord) -> Result<(), CardUpdateFailure> {
        check_change_in_rec(&self.snapshot, held)
    }

    /// The `CARD-UPDATE-RECORD` that `9200-WRITE-PROCESSING` rewrites over `held`, the card read
    /// for update (`COCRDUPC.cbl:1461-1483`; COCRDUPC-038, COCRDUPC-043).
    ///
    /// - The card number is `held`'s, the key read for update.
    /// - The account is the snapshot's, the account entered for the search or the list row's,
    ///   whatever account the card held (`X-COCRDUPC-2`).
    /// - The name is the name received, letter case kept.
    /// - The expiry date is the year received, the month received and the snapshot's day, typed.
    /// - The status is the status received.
    /// - The CVV and the 59 `FILLER` bytes are `held`'s, where the COBOL writes a CVV derived
    ///   from spaces and uninitialised filler: `HALTED(X-COCRDUPC-H2)`, `HALTED(X-COCRDUPC-H1)`;
    ///   keeping them is no parity claim.
    ///
    /// # Errors
    ///
    /// [`CardUpdateFailure::rewrite_failed`], before any write, when the record cannot be
    /// built: the year, month and stored day name no calendar date, such as month `02` of a card
    /// expiring on day 29 of a year that is no leap year, or the stored date is blank
    /// (`X-COCRDUPC-3`); or the snapshot's account is not 11 digits. Neither is a parity result.
    pub fn rewritten(&self, held: &CardRecord) -> Result<CardRecord, CardUpdateFailure> {
        let acct_id = self
            .snapshot
            .acct_id
            .ok_or(CardUpdateFailure::rewrite_failed())?;
        let expiraion_date = self
            .snapshot
            .expiry_day()
            .and_then(|day| {
                NaiveDate::from_ymd_opt(
                    self.changes.expiry_year(),
                    self.changes.expiry_month(),
                    day,
                )
            })
            .and_then(|date| PicDate::new(Some(date)).ok())
            .ok_or(CardUpdateFailure::rewrite_failed())?;
        Ok(CardRecord {
            num: held.num.clone(),
            acct_id,
            cvv_cd: held.cvv_cd.clone(),
            embossed_name: self.changes.embossed_name.clone(),
            expiraion_date,
            active_status: self.changes.status.clone(),
            filler: held.filler.clone(),
        })
    }

    /// The rewrite succeeded: [`CardUpdateOutcome::Committed`], state `C` (COCRDUPC-039).
    #[must_use]
    pub fn committed(&self) -> CardUpdateOutcome {
        CardUpdateOutcome::Committed {
            snapshot: self
                .snapshot
                .clone()
                .with_state(CardUpdateState::ChangesOkayedAndDone),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{PicDate, PicU, PicX};
    use proptest::prelude::*;

    use super::{
        ACCOUNT_FILTER_INVALID, Aid, CARD_EXPIRY_MONTH_NOT_VALID, CARD_EXPIRY_YEAR_NOT_VALID,
        CARD_FILTER_INVALID, CARD_STATUS_MUST_BE_YES_NO, CONFIRM_UPDATE_SUCCESS,
        COULD_NOT_LOCK_FOR_UPDATE, CardKey, CardLookup, CardSaveEffects, CardSnapshot,
        CardSnapshotFields, CardUpdateFailure, CardUpdateField, CardUpdateInput, CardUpdateOutcome,
        CardUpdateState, CardUpdateView, DATA_WAS_CHANGED_BEFORE_UPDATE,
        DID_NOT_FIND_ACCTCARD_COMBO, Decision, FILE_ERROR, FOUND_CARDS_FOR_ACCOUNT, FailureClass,
        INFORM_FAILURE, LOCKED_BUT_UPDATE_FAILED, NAME_MUST_BE_ALPHA, NO_CHANGES_DETECTED,
        NO_SEARCH_CRITERIA_RECEIVED, PROMPT_FOR_ACCT, PROMPT_FOR_CARD, PROMPT_FOR_CHANGES,
        PROMPT_FOR_CONFIRMATION, PROMPT_FOR_NAME, PROMPT_FOR_SEARCH_KEYS, check_change_in_rec,
        class_of, decide, edit, edit_keys, fetch, field_of,
    };
    use crate::online::cocrdlic::CardRef;
    use crate::online::cocrdslc;
    use crate::records::card::CardRecord;
    use crate::sensitive::{CardNumber, Cvv};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1111222233334444";

    /// An eleven-digit account with leading zeros, which differs from the card's own account.
    const ACCT: &str = "00000000050";

    /// A handle as a COCRDLI list row carries it.
    const HANDLE: CardRef = CardRef::new(7);

    /// The values the standard card is fetched with, as the screen sends them back unchanged.
    const AS_FETCHED: CardUpdateInput<'static> = CardUpdateInput {
        embossed_name: "TEST HOLDER",
        status: "Y",
        expiry_month: "04",
        expiry_year: "2025",
    };

    /// Changed values that pass every edit.
    const VALID_CHANGE: CardUpdateInput<'static> = CardUpdateInput {
        embossed_name: "New Holder",
        status: "N",
        expiry_month: "06",
        expiry_year: "2030",
    };

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| "not a calendar date".into())
    }

    /// A card of account 99 with CVV 321 and a `FILLER` that is not spaces.
    fn card(
        name: &str,
        expiry: Option<NaiveDate>,
        status: &str,
    ) -> Result<CardRecord, Box<dyn StdError>> {
        Ok(CardRecord {
            num: CardNumber::parse(CARD)?,
            acct_id: PicU::new(99)?,
            cvv_cd: Cvv::new(PicU::new(321)?),
            embossed_name: PicX::from_text(name)?,
            expiraion_date: PicDate::new(expiry)?,
            active_status: PicX::from_text(status)?,
            filler: PicX::from_text("KEPT FILLER")?,
        })
    }

    /// The standard card: `Test Holder`, expiring 2025-04-30, active.
    fn standard_card() -> Result<CardRecord, Box<dyn StdError>> {
        card("Test Holder", Some(date(2025, 4, 30)?), "Y")
    }

    /// Endpoint 9 for `record` on the list path, under account 50.
    fn shown(record: &CardRecord) -> Result<CardUpdateView, Box<dyn StdError>> {
        Ok(fetch(
            &CardKey::ByRef(HANDLE),
            CardLookup::Found {
                acct_id: PicU::new(50)?,
                card_ref: HANDLE,
                record,
            },
        )?)
    }

    /// The snapshot of the standard card, in `state`.
    fn snapshot_in(state: CardUpdateState) -> Result<CardSnapshot, Box<dyn StdError>> {
        Ok(shown(&standard_card()?)?.snapshot.with_state(state))
    }

    /// The save F5 confirms for `input` over `snapshot`.
    fn save_of(
        snapshot: &CardSnapshot,
        input: &CardUpdateInput<'_>,
    ) -> Result<CardSaveEffects, Box<dyn StdError>> {
        match decide(Aid::Pf5, snapshot, input)? {
            Decision::Save(effects) => Ok(effects),
            Decision::Respond(outcome) => Err(format!("no save: {outcome:?}").into()),
        }
    }

    /// Asserts that `failure` is the 400 of an edit or of the change check.
    fn assert_input(
        failure: Option<CardUpdateFailure>,
        message: &str,
        field: CardUpdateField,
        state: CardUpdateState,
    ) {
        assert_eq!(failure.map(|f| f.message()), Some(message));
        assert_eq!(failure.and_then(|f| f.field()), Some(field), "{message}");
        assert_eq!(failure.map(|f| f.class()), Some(FailureClass::Input));
        assert_eq!(failure.map(|f| f.class().http_status()), Some(400));
        assert_eq!(failure.map(|f| f.state()), Some(state), "{message}");
        assert_eq!(failure.map(|f| f.to_string()), Some(message.to_owned()));
    }

    // audit: COCRDUPC-002
    // audit: COCRDUPC-016
    // audit: COCRDUPC-017
    // audit: COCRDUPC-042
    // audit: COCRDUPC-044
    #[test]
    fn a_fetched_card_is_shown_upper_cased_and_redacted() -> TestResult {
        let record = standard_card()?;
        for key in [
            CardKey::ByRef(HANDLE),
            CardKey::ByNumber(CardNumber::parse(CARD)?),
        ] {
            let view = fetch(
                &key,
                CardLookup::Found {
                    acct_id: PicU::new(50)?,
                    card_ref: HANDLE,
                    record: &record,
                },
            )?;
            assert_eq!(view.card_ref, HANDLE);
            assert_eq!(view.acct_id, PicU::new(50)?);
            assert_ne!(view.acct_id, record.acct_id);
            assert_eq!(view.card_num.redacted(), "************4444");
            assert_eq!(view.embossed_name, PicX::from_text("TEST HOLDER")?);
            assert_eq!(view.expiration_date, Some(date(2025, 4, 30)?));
            assert_eq!(view.status, PicX::from_text("Y")?);
            assert_eq!(view.info_message, FOUND_CARDS_FOR_ACCOUNT);
            let snapshot = &view.snapshot;
            assert_eq!(snapshot.state(), CardUpdateState::ShowDetails);
            assert_eq!(snapshot.acct_id(), Some(PicU::new(50)?));
            assert_eq!(snapshot.embossed_name(), &view.embossed_name);
            assert_eq!(snapshot.expiration_date(), view.expiration_date);
            assert_eq!(snapshot.status(), &view.status);
            let rendered = format!("{view:?}");
            assert!(
                !rendered.contains(CARD),
                "Debug shows the redacted number only"
            );
            assert!(rendered.contains("************4444"));
            assert!(!rendered.contains("321"), "no CVV is shown");
        }
        Ok(())
    }

    // audit: COCRDUPC-003
    // audit: COCRDUPC-018
    // audit: COCRDUPC-019
    // audit: COCRDUPC-047
    #[test]
    fn a_failed_read_names_the_account_in_the_state_of_its_path() -> TestResult {
        let by_number = CardKey::ByNumber(CardNumber::parse(CARD)?);
        let cases = [
            (CardKey::ByRef(HANDLE), CardUpdateState::ShowDetails),
            (by_number, CardUpdateState::NotFetched),
        ];
        for (key, state) in cases {
            let missing = fetch(&key, CardLookup::NotFound).err();
            assert_eq!(
                missing.map(|f| f.message()),
                Some(DID_NOT_FIND_ACCTCARD_COMBO)
            );
            assert_eq!(missing.map(|f| f.class().http_status()), Some(404));
            assert_eq!(
                missing.and_then(|f| f.field()),
                Some(CardUpdateField::AcctId)
            );
            assert_eq!(missing.map(|f| f.state()), Some(state));
            let failed = fetch(&key, CardLookup::Error).err();
            assert_eq!(failed.map(|f| f.message()), Some(FILE_ERROR));
            assert_eq!(failed.map(|f| f.class()), Some(FailureClass::Store));
            assert_eq!(failed.map(|f| f.class().http_status()), Some(500));
            assert_eq!(
                failed.and_then(|f| f.field()),
                Some(CardUpdateField::AcctId)
            );
            assert_eq!(failed.map(|f| f.state()), Some(state));
        }
        assert_eq!(
            DID_NOT_FIND_ACCTCARD_COMBO,
            "Did not find cards for this search condition"
        );
        // The fixed bytes of the 75-byte text; bytes 49-58 and 66-75 are the CICS codes,
        // BLOCKED by HALTED(H4), and no value is asserted for them.
        assert_eq!(FILE_ERROR.len(), 75);
        assert_eq!(
            FILE_ERROR.get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(FILE_ERROR.get(58..65), Some(",RESP2 "));
        Ok(())
    }

    // audit: COCRDUPC-002
    #[test]
    fn the_list_path_takes_both_keys_as_valid() -> TestResult {
        let accepted = [
            (ACCT, None, PicU::new(50)?),
            ("00000000000", None, PicU::ZERO),
            (ACCT, Some("                "), PicU::new(50)?),
            (ACCT, Some("*"), PicU::new(50)?),
        ];
        for (acct, card_num, expected) in accepted {
            assert_eq!(
                edit_keys(acct, card_num, Some(HANDLE)),
                Ok((expected, CardKey::ByRef(HANDLE)))
            );
        }
        // A request no list row sends is edited as a search.
        let typed_beside_handle = edit_keys(ACCT, Some(CARD), Some(HANDLE)).err();
        assert_input(
            typed_beside_handle,
            CARD_FILTER_INVALID,
            CardUpdateField::CardNum,
            CardUpdateState::NotFetched,
        );
        let short_account = edit_keys("50", None, Some(HANDLE)).err();
        assert_input(
            short_account,
            ACCOUNT_FILTER_INVALID,
            CardUpdateField::AcctId,
            CardUpdateState::NotFetched,
        );
        Ok(())
    }

    // audit: COCRDUPC-010
    // audit: COCRDUPC-011
    // audit: COCRDUPC-012
    // audit: COCRDUPC-013
    // audit: COCRDUPC-014
    // audit: COCRDUPC-015
    #[test]
    fn each_search_key_failure_has_its_message_and_field() -> TestResult {
        let cases: [(&str, Option<&str>, &str, CardUpdateField); 8] = [
            (
                "",
                None,
                NO_SEARCH_CRITERIA_RECEIVED,
                CardUpdateField::AcctId,
            ),
            (
                "*",
                Some("   "),
                NO_SEARCH_CRITERIA_RECEIVED,
                CardUpdateField::AcctId,
            ),
            (
                "00000000000",
                Some(CARD),
                PROMPT_FOR_ACCT,
                CardUpdateField::AcctId,
            ),
            (
                "1234",
                Some(CARD),
                ACCOUNT_FILTER_INVALID,
                CardUpdateField::AcctId,
            ),
            (
                "1234567890A",
                None,
                ACCOUNT_FILTER_INVALID,
                CardUpdateField::AcctId,
            ),
            (
                ACCT,
                Some("0000000000000000"),
                PROMPT_FOR_CARD,
                CardUpdateField::CardNum,
            ),
            (ACCT, None, PROMPT_FOR_CARD, CardUpdateField::CardNum),
            (
                ACCT,
                Some("111122223333444X"),
                CARD_FILTER_INVALID,
                CardUpdateField::CardNum,
            ),
        ];
        for (acct, card_num, message, field) in cases {
            let failure = edit_keys(acct, card_num, None).err();
            assert_input(failure, message, field, CardUpdateState::NotFetched);
            assert_eq!(
                failure.map(|f| f.state().info_message()),
                Some(PROMPT_FOR_SEARCH_KEYS)
            );
        }
        assert_eq!(
            edit_keys(ACCT, Some(CARD), None),
            Ok((PicU::new(50)?, CardKey::ByNumber(CardNumber::parse(CARD)?)))
        );
        // COCRDUPC's own literals, which the shared definitions must hold byte for byte.
        assert_eq!(PROMPT_FOR_ACCT, "Account number not provided");
        assert_eq!(PROMPT_FOR_CARD, "Card number not provided");
        assert_eq!(NO_SEARCH_CRITERIA_RECEIVED, "No input received");
        assert_eq!(
            ACCOUNT_FILTER_INVALID,
            "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER"
        );
        assert_eq!(
            CARD_FILTER_INVALID,
            "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER"
        );
        Ok(())
    }

    // audit: COCRDUPC-020
    // audit: COCRDUPC-021
    // audit: COCRDUPC-022
    #[test]
    fn values_equal_to_the_fetched_ones_once_upper_cased_are_no_change() -> TestResult {
        let case_only = CardUpdateInput {
            embossed_name: "test Holder",
            status: "y",
            ..AS_FETCHED
        };
        for (state, info) in [
            (CardUpdateState::ShowDetails, FOUND_CARDS_FOR_ACCOUNT),
            (CardUpdateState::ChangesNotOk, PROMPT_FOR_CHANGES),
        ] {
            for input in [AS_FETCHED, case_only] {
                for aid in [Aid::Enter, Aid::Pf5] {
                    let failure = decide(aid, &snapshot_in(state)?, &input).err();
                    assert_input(
                        failure,
                        NO_CHANGES_DETECTED,
                        CardUpdateField::EmbossedName,
                        state,
                    );
                    assert_eq!(failure.map(|f| f.state().info_message()), Some(info));
                }
            }
        }
        assert_eq!(
            NO_CHANGES_DETECTED,
            "No change detected with respect to values fetched."
        );
        Ok(())
    }

    // audit: COCRDUPC-009
    #[test]
    fn a_blank_or_asterisk_field_is_low_values_and_always_a_change() -> TestResult {
        // A stored status of a space: the screen sends it back blank, which becomes low-values
        // and so differs from the space fetched; the status edit then fails.
        let record = card("Test Holder", Some(date(2025, 4, 30)?), " ")?;
        let snapshot = shown(&record)?.snapshot;
        for status in ["", " ", "*"] {
            let blank_status = CardUpdateInput {
                status,
                ..AS_FETCHED
            };
            let failure = decide(Aid::Enter, &snapshot, &blank_status).err();
            assert_input(
                failure,
                CARD_STATUS_MUST_BE_YES_NO,
                CardUpdateField::Status,
                CardUpdateState::ChangesNotOk,
            );
        }
        let asterisk_name = CardUpdateInput {
            embossed_name: "*   ",
            ..AS_FETCHED
        };
        let failure = decide(
            Aid::Enter,
            &snapshot_in(CardUpdateState::ShowDetails)?,
            &asterisk_name,
        )
        .err();
        assert_input(
            failure,
            PROMPT_FOR_NAME,
            CardUpdateField::EmbossedName,
            CardUpdateState::ChangesNotOk,
        );
        Ok(())
    }

    /// 51 letters: one byte more than `CRDNAMEI` holds.
    const LONG_NAME: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    /// 50 zeros, which `1230-EDIT-NAME` takes as not supplied.
    const ZERO_NAME: &str = "00000000000000000000000000000000000000000000000000";

    /// One failing value per row: the field it goes in, the text and the message it gets.
    const EDIT_FAILURES: [(CardUpdateField, &str, &str); 24] = [
        (CardUpdateField::EmbossedName, "", PROMPT_FOR_NAME),
        (CardUpdateField::EmbossedName, "*", PROMPT_FOR_NAME),
        (CardUpdateField::EmbossedName, ZERO_NAME, PROMPT_FOR_NAME),
        (
            CardUpdateField::EmbossedName,
            "John 3rd",
            NAME_MUST_BE_ALPHA,
        ),
        (CardUpdateField::EmbossedName, "O'Hara", NAME_MUST_BE_ALPHA),
        (
            CardUpdateField::EmbossedName,
            "Ann\tLee",
            NAME_MUST_BE_ALPHA,
        ),
        (
            CardUpdateField::EmbossedName,
            "Zo\u{eb}",
            NAME_MUST_BE_ALPHA,
        ),
        (CardUpdateField::EmbossedName, LONG_NAME, NAME_MUST_BE_ALPHA),
        (CardUpdateField::Status, "", CARD_STATUS_MUST_BE_YES_NO),
        (CardUpdateField::Status, "0", CARD_STATUS_MUST_BE_YES_NO),
        (CardUpdateField::Status, "y", CARD_STATUS_MUST_BE_YES_NO),
        (CardUpdateField::Status, "n", CARD_STATUS_MUST_BE_YES_NO),
        (CardUpdateField::Status, "X", CARD_STATUS_MUST_BE_YES_NO),
        (CardUpdateField::Status, "YN", CARD_STATUS_MUST_BE_YES_NO),
        (
            CardUpdateField::ExpiryMonth,
            "",
            CARD_EXPIRY_MONTH_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryMonth,
            "00",
            CARD_EXPIRY_MONTH_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryMonth,
            "13",
            CARD_EXPIRY_MONTH_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryMonth,
            "012",
            CARD_EXPIRY_MONTH_NOT_VALID,
        ),
        (CardUpdateField::ExpiryYear, "", CARD_EXPIRY_YEAR_NOT_VALID),
        (
            CardUpdateField::ExpiryYear,
            "0000",
            CARD_EXPIRY_YEAR_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryYear,
            "1949",
            CARD_EXPIRY_YEAR_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryYear,
            "2100",
            CARD_EXPIRY_YEAR_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryYear,
            "0025",
            CARD_EXPIRY_YEAR_NOT_VALID,
        ),
        (
            CardUpdateField::ExpiryYear,
            "20255",
            CARD_EXPIRY_YEAR_NOT_VALID,
        ),
    ];

    /// [`VALID_CHANGE`] with `text` in `field`.
    fn changed(field: CardUpdateField, text: &str) -> CardUpdateInput<'_> {
        match field {
            CardUpdateField::EmbossedName => CardUpdateInput {
                embossed_name: text,
                ..VALID_CHANGE
            },
            CardUpdateField::Status => CardUpdateInput {
                status: text,
                ..VALID_CHANGE
            },
            CardUpdateField::ExpiryMonth => CardUpdateInput {
                expiry_month: text,
                ..VALID_CHANGE
            },
            CardUpdateField::ExpiryYear => CardUpdateInput {
                expiry_year: text,
                ..VALID_CHANGE
            },
            CardUpdateField::AcctId | CardUpdateField::CardNum => VALID_CHANGE,
        }
    }

    // audit: COCRDUPC-023
    // audit: COCRDUPC-024
    // audit: COCRDUPC-025
    // audit: COCRDUPC-027
    // audit: COCRDUPC-028
    // audit: COCRDUPC-029
    // audit: COCRDUPC-030
    // audit: COCRDUPC-031
    // audit: COCRDUPC-032
    #[test]
    fn each_detail_edit_failure_has_its_message_and_field() -> TestResult {
        assert_eq!(LONG_NAME.len(), 51);
        assert_eq!(ZERO_NAME.len(), 50);
        let snapshot = snapshot_in(CardUpdateState::ShowDetails)?;
        for (field, text, message) in EDIT_FAILURES {
            let input = changed(field, text);
            assert_input(
                edit(&input).err(),
                message,
                field,
                CardUpdateState::ChangesNotOk,
            );
            let decided = decide(Aid::Enter, &snapshot, &input).err();
            assert_input(decided, message, field, CardUpdateState::ChangesNotOk);
            assert_eq!(
                decided.map(|f| f.state().info_message()),
                Some(PROMPT_FOR_CHANGES)
            );
        }
        Ok(())
    }

    // audit: COCRDUPC-023
    #[test]
    fn the_first_failing_edit_decides_the_message_and_the_field() {
        let all_bad = CardUpdateInput {
            embossed_name: "",
            status: "x",
            expiry_month: "13",
            expiry_year: "1900",
        };
        let precedence = [
            (all_bad, PROMPT_FOR_NAME, CardUpdateField::EmbossedName),
            (
                CardUpdateInput {
                    embossed_name: "Fine Name",
                    ..all_bad
                },
                CARD_STATUS_MUST_BE_YES_NO,
                CardUpdateField::Status,
            ),
            (
                CardUpdateInput {
                    embossed_name: "Fine Name",
                    status: "Y",
                    ..all_bad
                },
                CARD_EXPIRY_MONTH_NOT_VALID,
                CardUpdateField::ExpiryMonth,
            ),
            (
                CardUpdateInput {
                    expiry_year: "1900",
                    ..VALID_CHANGE
                },
                CARD_EXPIRY_YEAR_NOT_VALID,
                CardUpdateField::ExpiryYear,
            ),
        ];
        for (input, message, field) in precedence {
            assert_input(
                edit(&input).err(),
                message,
                field,
                CardUpdateState::ChangesNotOk,
            );
        }
    }

    // audit: COCRDUPC-025
    // audit: COCRDUPC-028
    // audit: COCRDUPC-030
    // audit: COCRDUPC-032
    #[test]
    fn the_bounds_of_each_edit_pass() -> TestResult {
        let passing = [
            ("Mary Ann", "N", "01", "1950", 1, 1950),
            ("ZZ", "Y", "12", "2099", 12, 2099),
            ("lower case name", "Y", "09", "2025", 9, 2025),
        ];
        for (name, status, month, year, month_value, year_value) in passing {
            let input = CardUpdateInput {
                embossed_name: name,
                status,
                expiry_month: month,
                expiry_year: year,
            };
            let changes = edit(&input)?;
            assert_eq!(changes.embossed_name(), &PicX::from_text(name)?);
            assert_eq!(changes.status(), &PicX::from_text(status)?);
            assert_eq!(changes.expiry_month(), month_value);
            assert_eq!(changes.expiry_year(), year_value);
        }
        Ok(())
    }

    /// The months and years that hold a non-digit, which the COBOL tests through a numeric
    /// redefinition: BLOCKED by `HALTED(X-COCRDUPC-H2)`. The replacement's answer is asserted,
    /// with no parity claim and no audit tag.
    #[test]
    fn a_non_digit_month_or_year_fails_its_own_edit() {
        for month in ["0A", "3", " 3", "1.", "\0\0"] {
            let input = changed(CardUpdateField::ExpiryMonth, month);
            assert_eq!(
                edit(&input).err().map(|f| f.message()),
                Some(CARD_EXPIRY_MONTH_NOT_VALID)
            );
        }
        for year in ["2O25", "25", " 025", "+025"] {
            let input = changed(CardUpdateField::ExpiryYear, year);
            assert_eq!(
                edit(&input).err().map(|f| f.message()),
                Some(CARD_EXPIRY_YEAR_NOT_VALID)
            );
        }
    }

    // audit: COCRDUPC-026
    #[test]
    fn an_untouched_stored_name_that_fails_the_alphabet_edit_blocks_the_save() -> TestResult {
        let record = card("Pat O'Neil", Some(date(2025, 4, 30)?), "Y")?;
        let snapshot = shown(&record)?.snapshot;
        let status_only = CardUpdateInput {
            embossed_name: "PAT O'NEIL",
            status: "N",
            expiry_month: "04",
            expiry_year: "2025",
        };
        let failure = decide(Aid::Enter, &snapshot, &status_only).err();
        assert_input(
            failure,
            NAME_MUST_BE_ALPHA,
            CardUpdateField::EmbossedName,
            CardUpdateState::ChangesNotOk,
        );
        // Unchanged, the same card gets the no-change answer instead: the change check runs
        // before the edits.
        let untouched = CardUpdateInput {
            status: "Y",
            ..status_only
        };
        let failure = decide(Aid::Enter, &snapshot, &untouched).err();
        assert_eq!(failure.map(|f| f.message()), Some(NO_CHANGES_DETECTED));
        Ok(())
    }

    // audit: COCRDUPC-007
    // audit: COCRDUPC-034
    #[test]
    fn a_valid_change_asks_for_confirmation_and_writes_nothing() -> TestResult {
        for state in [CardUpdateState::ShowDetails, CardUpdateState::ChangesNotOk] {
            let snapshot = snapshot_in(state)?;
            for aid in [Aid::Enter, Aid::Pf5] {
                let decision = decide(aid, &snapshot, &VALID_CHANGE)?;
                let expected = CardUpdateOutcome::Validated {
                    snapshot: snapshot
                        .clone()
                        .with_state(CardUpdateState::ChangesOkNotConfirmed),
                };
                assert_eq!(decision, Decision::Respond(expected.clone()));
                assert_eq!(expected.state(), CardUpdateState::ChangesOkNotConfirmed);
                assert_eq!(expected.info_message(), PROMPT_FOR_CONFIRMATION);
                assert!(expected.confirmation_keys_shown());
                let echoed = expected.snapshot().ok_or("a snapshot is returned")?;
                assert_eq!(echoed.embossed_name(), snapshot.embossed_name());
                assert_eq!(echoed.expiration_date(), snapshot.expiration_date());
            }
        }
        Ok(())
    }

    // audit: COCRDUPC-035
    // audit: COCRDUPC-041
    #[test]
    fn state_n_validates_on_enter_and_saves_on_f5_after_editing_again() -> TestResult {
        let snapshot = snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?;
        // ENTER in state N runs no edit: even values that would fail are taken as validated.
        let unedited = CardUpdateInput {
            embossed_name: "1",
            status: "?",
            expiry_month: "99",
            expiry_year: "1",
        };
        let validated = Decision::Respond(CardUpdateOutcome::Validated {
            snapshot: snapshot.clone(),
        });
        assert_eq!(decide(Aid::Enter, &snapshot, &unedited)?, validated);
        let unchanged = decide(Aid::Enter, &snapshot, &AS_FETCHED).err();
        assert_input(
            unchanged,
            NO_CHANGES_DETECTED,
            CardUpdateField::EmbossedName,
            CardUpdateState::ChangesOkNotConfirmed,
        );
        // F5 in state N edits the fields received again before it saves.
        let effects = save_of(&snapshot, &VALID_CHANGE)?;
        assert_eq!(effects.snapshot(), &snapshot);
        assert_eq!(effects.changes(), &edit(&VALID_CHANGE)?);
        let refused = decide(Aid::Pf5, &snapshot, &unedited).err();
        assert_input(
            refused,
            NAME_MUST_BE_ALPHA,
            CardUpdateField::EmbossedName,
            CardUpdateState::ChangesNotOk,
        );
        let no_change = decide(Aid::Pf5, &snapshot, &AS_FETCHED).err();
        assert_input(
            no_change,
            NO_CHANGES_DETECTED,
            CardUpdateField::EmbossedName,
            CardUpdateState::ShowDetails,
        );
        Ok(())
    }

    /// COCRDUPC-005 is the screen's: the next key after a save, a failure, or before a fetch
    /// starts a new search; endpoint 10 answers an echoed state of that kind the same way.
    #[test]
    fn states_after_a_save_or_before_a_fetch_restart_the_search() -> TestResult {
        for state in [
            CardUpdateState::NotFetched,
            CardUpdateState::ChangesOkayedAndDone,
            CardUpdateState::LockError,
            CardUpdateState::UpdateFailed,
        ] {
            for aid in [Aid::Enter, Aid::Pf5] {
                let decision = decide(aid, &snapshot_in(state)?, &VALID_CHANGE)?;
                assert_eq!(decision, Decision::Respond(CardUpdateOutcome::Restart));
            }
        }
        let restart = CardUpdateOutcome::Restart;
        assert_eq!(restart.state(), CardUpdateState::NotFetched);
        assert_eq!(restart.info_message(), PROMPT_FOR_SEARCH_KEYS);
        assert!(!restart.confirmation_keys_shown());
        assert_eq!(restart.snapshot(), None);
        Ok(())
    }

    // audit: COCRDUPC-037
    #[test]
    fn a_card_changed_since_the_fetch_is_a_conflict() -> TestResult {
        let snapshot = snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?;
        let effects = save_of(&snapshot, &VALID_CHANGE)?;
        let same = standard_card()?;
        assert_eq!(check_change_in_rec(&snapshot, &same), Ok(()));
        assert_eq!(effects.check_change_in_rec(&same), Ok(()));
        // A change of letter case alone, or of the CVV alone, is not detected.
        let case_only = card("TEST holder", Some(date(2025, 4, 30)?), "Y")?;
        assert_eq!(check_change_in_rec(&snapshot, &case_only), Ok(()));
        let cvv_only = CardRecord {
            cvv_cd: Cvv::new(PicU::new(999)?),
            ..standard_card()?
        };
        assert_eq!(effects.check_change_in_rec(&cvv_only), Ok(()));
        let changed = [
            card("Other Holder", Some(date(2025, 4, 30)?), "Y")?,
            card("Test Holder", Some(date(2025, 4, 29)?), "Y")?,
            card("Test Holder", Some(date(2026, 4, 30)?), "Y")?,
            card("Test Holder", None, "Y")?,
            card("Test Holder", Some(date(2025, 4, 30)?), "N")?,
        ];
        for current in &changed {
            let conflict = effects.check_change_in_rec(current).err();
            assert_eq!(
                conflict.map(|f| f.message()),
                Some(DATA_WAS_CHANGED_BEFORE_UPDATE)
            );
            assert_eq!(conflict.map(|f| f.class()), Some(FailureClass::Conflict));
            assert_eq!(conflict.map(|f| f.class().http_status()), Some(409));
            assert_eq!(conflict.and_then(|f| f.field()), None);
            assert_eq!(
                conflict.map(|f| f.state()),
                Some(CardUpdateState::ShowDetails)
            );
            assert_eq!(
                conflict.map(|f| f.state().info_message()),
                Some(FOUND_CARDS_FOR_ACCOUNT)
            );
        }
        Ok(())
    }

    // audit: COCRDUPC-038
    // audit: COCRDUPC-043
    #[test]
    fn the_rewrite_takes_the_received_values_and_keeps_the_key_cvv_and_filler() -> TestResult {
        let held = standard_card()?;
        let snapshot = snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?;
        let effects = save_of(&snapshot, &VALID_CHANGE)?;
        let rewritten = effects.rewritten(&held)?;
        assert_eq!(rewritten.num, held.num);
        // The account entered for the search, never the card's own (X-COCRDUPC-2).
        assert_eq!(rewritten.acct_id, PicU::new(50)?);
        assert_eq!(rewritten.cvv_cd, held.cvv_cd);
        assert_eq!(rewritten.embossed_name, PicX::from_text("New Holder")?);
        assert_eq!(rewritten.expiraion_date.value(), Some(date(2030, 6, 30)?));
        assert_eq!(rewritten.active_status, PicX::from_text("N")?);
        assert_eq!(rewritten.filler, held.filler);
        // An untouched name is written back upper-cased, as fetched.
        let status_only = CardUpdateInput {
            status: "N",
            ..AS_FETCHED
        };
        let upper_cased = save_of(&snapshot, &status_only)?.rewritten(&held)?;
        assert_eq!(upper_cased.embossed_name, PicX::from_text("TEST HOLDER")?);
        Ok(())
    }

    /// Year, month and stored day that name no calendar date, a blank stored date and an
    /// unknown account: the replacement's rewrite failure (`X-COCRDUPC-3`), no parity claim.
    #[test]
    fn a_record_that_cannot_be_built_is_a_rewrite_failure() -> TestResult {
        let day_29 = card("Test Holder", Some(date(2024, 1, 29)?), "Y")?;
        let february = CardUpdateInput {
            expiry_month: "02",
            expiry_year: "2025",
            ..VALID_CHANGE
        };
        let snapshot = shown(&day_29)?
            .snapshot
            .with_state(CardUpdateState::ChangesOkNotConfirmed);
        let no_such_day = save_of(&snapshot, &february)?.rewritten(&day_29);
        assert_eq!(no_such_day, Err(CardUpdateFailure::rewrite_failed()));
        let blank_date = card("Test Holder", None, "Y")?;
        let snapshot = shown(&blank_date)?
            .snapshot
            .with_state(CardUpdateState::ChangesOkNotConfirmed);
        let no_day = save_of(&snapshot, &VALID_CHANGE)?.rewritten(&blank_date);
        assert_eq!(no_day, Err(CardUpdateFailure::rewrite_failed()));
        let unknown_account = CardSnapshot::from_fields(&CardSnapshotFields {
            state: CardUpdateState::ChangesOkNotConfirmed,
            acct_id: "5O",
            embossed_name: "TEST HOLDER",
            expiration_date: Some(date(2025, 4, 30)?),
            status: "Y",
        });
        let no_account = save_of(&unknown_account, &VALID_CHANGE)?.rewritten(&standard_card()?);
        assert_eq!(no_account, Err(CardUpdateFailure::rewrite_failed()));
        let far_year = NaiveDate::from_ymd_opt(10_000, 4, 30).ok_or("a chrono date")?;
        let unknown_date = CardSnapshot::from_fields(&CardSnapshotFields {
            state: CardUpdateState::ChangesOkNotConfirmed,
            acct_id: ACCT,
            embossed_name: "TEST HOLDER",
            expiration_date: Some(far_year),
            status: "Y",
        });
        let no_date = save_of(&unknown_date, &VALID_CHANGE)?.rewritten(&standard_card()?);
        assert_eq!(no_date, Err(CardUpdateFailure::rewrite_failed()));
        Ok(())
    }

    // audit: COCRDUPC-036
    // audit: COCRDUPC-039
    // audit: COCRDUPC-040
    #[test]
    fn each_save_outcome_has_its_status_state_and_info_line() -> TestResult {
        let lock = CardUpdateFailure::lock_failed();
        assert_eq!(lock.message(), COULD_NOT_LOCK_FOR_UPDATE);
        assert_eq!(lock.message(), "Could not lock record for update");
        assert_eq!(lock.state(), CardUpdateState::LockError);
        let rewrite = CardUpdateFailure::rewrite_failed();
        assert_eq!(rewrite.message(), LOCKED_BUT_UPDATE_FAILED);
        assert_eq!(rewrite.message(), "Update of record failed");
        assert_eq!(rewrite.state(), CardUpdateState::UpdateFailed);
        for failure in [lock, rewrite] {
            assert_eq!(failure.class(), FailureClass::Store);
            assert_eq!(failure.class().http_status(), 500);
            assert_eq!(failure.field(), None);
            assert_eq!(failure.state().info_message(), INFORM_FAILURE);
            assert_eq!(INFORM_FAILURE, "Changes unsuccessful. Please try again");
        }
        let snapshot = snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?;
        let committed = save_of(&snapshot, &VALID_CHANGE)?.committed();
        assert_eq!(committed.state(), CardUpdateState::ChangesOkayedAndDone);
        assert_eq!(committed.info_message(), CONFIRM_UPDATE_SUCCESS);
        assert_eq!(committed.info_message(), "Changes committed to database");
        assert!(!committed.confirmation_keys_shown());
        assert_eq!(
            committed.snapshot().map(CardSnapshot::state),
            Some(CardUpdateState::ChangesOkayedAndDone)
        );
        Ok(())
    }

    // audit: COCRDUPC-044
    #[test]
    fn the_snapshot_travels_as_text_and_holds_unknown_values_as_unknown() -> TestResult {
        let snapshot = snapshot_in(CardUpdateState::ChangesNotOk)?;
        let name = snapshot.embossed_name().to_text();
        let status = snapshot.status().to_text();
        let echoed = CardSnapshot::from_fields(&CardSnapshotFields {
            state: snapshot.state(),
            acct_id: ACCT,
            embossed_name: &name,
            expiration_date: snapshot.expiration_date(),
            status: &status,
        });
        assert_eq!(echoed, snapshot);
        let far_year = NaiveDate::from_ymd_opt(10_000, 1, 1).ok_or("a chrono date")?;
        let unknown = CardSnapshot::from_fields(&CardSnapshotFields {
            state: CardUpdateState::ShowDetails,
            acct_id: "123",
            embossed_name: &name,
            expiration_date: Some(far_year),
            status: &status,
        });
        assert_eq!(unknown.acct_id(), None);
        assert_eq!(unknown.expiration_date(), None);
        // An unknown date equals no stored one: every request reads as a change, and the card
        // read for update as changed by another user.
        let decision = decide(Aid::Enter, &unknown, &AS_FETCHED)?;
        assert!(matches!(
            decision,
            Decision::Respond(CardUpdateOutcome::Validated { .. })
        ));
        let conflict = check_change_in_rec(&unknown, &standard_card()?).err();
        assert_eq!(conflict.map(|f| f.class()), Some(FailureClass::Conflict));
        Ok(())
    }

    #[test]
    fn state_codes_and_info_lines_follow_the_program() {
        let states = [
            (CardUpdateState::NotFetched, ' ', PROMPT_FOR_SEARCH_KEYS),
            (CardUpdateState::ShowDetails, 'S', FOUND_CARDS_FOR_ACCOUNT),
            (CardUpdateState::ChangesNotOk, 'E', PROMPT_FOR_CHANGES),
            (
                CardUpdateState::ChangesOkNotConfirmed,
                'N',
                PROMPT_FOR_CONFIRMATION,
            ),
            (
                CardUpdateState::ChangesOkayedAndDone,
                'C',
                CONFIRM_UPDATE_SUCCESS,
            ),
            (CardUpdateState::LockError, 'L', INFORM_FAILURE),
            (CardUpdateState::UpdateFailed, 'F', INFORM_FAILURE),
        ];
        for (state, code, info) in states {
            assert_eq!(state.code(), code);
            assert_eq!(CardUpdateState::from_code(&code.to_string()), Some(state));
            assert_eq!(state.info_message(), info);
            assert_eq!(
                state.confirmation_keys_shown(),
                state == CardUpdateState::ChangesOkNotConfirmed
            );
        }
        for not_fetched in ["", "\0"] {
            assert_eq!(
                CardUpdateState::from_code(not_fetched),
                Some(CardUpdateState::NotFetched)
            );
        }
        for unknown in ["X", "s", "SS", "  "] {
            assert_eq!(CardUpdateState::from_code(unknown), None);
        }
        assert_eq!(
            FOUND_CARDS_FOR_ACCOUNT,
            "Details of selected card shown above"
        );
        assert_eq!(
            PROMPT_FOR_SEARCH_KEYS,
            "Please enter Account and Card Number"
        );
        assert_eq!(PROMPT_FOR_CHANGES, "Update card details presented above.");
        assert_eq!(
            PROMPT_FOR_CONFIRMATION,
            "Changes validated.Press F5 to save"
        );
    }

    #[test]
    fn fields_and_classes_map_to_the_response() {
        let names = [
            (CardUpdateField::AcctId, "acct_id"),
            (CardUpdateField::CardNum, "card_num"),
            (CardUpdateField::EmbossedName, "embossed_name"),
            (CardUpdateField::Status, "status"),
            (CardUpdateField::ExpiryMonth, "expiry_month"),
            (CardUpdateField::ExpiryYear, "expiry_year"),
        ];
        for (field, name) in names {
            assert_eq!(field.name(), name);
        }
        assert_eq!(
            field_of(cocrdslc::DetailField::AcctId),
            CardUpdateField::AcctId
        );
        assert_eq!(
            field_of(cocrdslc::DetailField::CardNum),
            CardUpdateField::CardNum
        );
        let classes = [
            (cocrdslc::FailureClass::Input, FailureClass::Input, 400),
            (
                cocrdslc::FailureClass::NotFound,
                FailureClass::NotFound,
                404,
            ),
            (cocrdslc::FailureClass::Store, FailureClass::Store, 500),
        ];
        for (shared, own, status) in classes {
            assert_eq!(class_of(shared), own);
            assert_eq!(own.http_status(), status);
        }
        assert_eq!(FailureClass::Conflict.http_status(), 409);
    }

    // audit: COCRDUPC-042
    #[test]
    fn no_answer_or_effect_shows_the_card_number() -> TestResult {
        let snapshot = snapshot_in(CardUpdateState::ChangesOkNotConfirmed)?;
        let effects = save_of(&snapshot, &VALID_CHANGE)?;
        let rendered = [
            format!("{:?}", shown(&standard_card()?)?),
            format!("{effects:?}"),
            format!("{:?}", effects.rewritten(&standard_card()?)?),
            format!("{:?}", effects.committed()),
            format!("{:?}", edit_keys(ACCT, Some(CARD), None)),
            format!("{:?}", edit_keys(ACCT, Some("111122223333444X"), None)),
        ];
        for text in &rendered {
            assert!(
                !text.contains(CARD),
                "no full card number in {}",
                text.len()
            );
            assert!(!text.contains("111122223333444X"), "no input echoed");
        }
        Ok(())
    }

    fn any_state() -> impl Strategy<Value = CardUpdateState> {
        prop_oneof![
            Just(CardUpdateState::NotFetched),
            Just(CardUpdateState::ShowDetails),
            Just(CardUpdateState::ChangesNotOk),
            Just(CardUpdateState::ChangesOkNotConfirmed),
            Just(CardUpdateState::ChangesOkayedAndDone),
            Just(CardUpdateState::LockError),
            Just(CardUpdateState::UpdateFailed),
        ]
    }

    proptest! {
        /// Every function over request text is total: any text of any length gives a value or a
        /// failure, never a panic (Total Parsers Over Untrusted Input). The second alternative
        /// of each detail strategy draws values that pass their edit, so saves are reached too.
        #[test]
        fn every_request_text_gets_an_answer(
            acct in prop_oneof![".{0,14}", "[0-9]{11}"],
            card_num in proptest::option::of(".{0,20}"),
            handle in proptest::option::of(any::<i64>()),
            name in prop_oneof![".{0,60}", "[A-Za-z ]{1,50}"],
            status in prop_oneof![".{0,3}", "[YN]"],
            month in prop_oneof![".{0,4}", "(0[1-9]|1[0-2])"],
            year in prop_oneof![".{0,6}", "(19[5-9][0-9]|20[0-9]{2})"],
            state in any_state(),
            pf5 in any::<bool>(),
        ) {
            let _ = edit_keys(&acct, card_num.as_deref(), handle.map(CardRef::new));
            let input = CardUpdateInput {
                embossed_name: &name,
                status: &status,
                expiry_month: &month,
                expiry_year: &year,
            };
            let _ = edit(&input);
            let expiry = NaiveDate::from_ymd_opt(2025, 4, 30);
            let snapshot = CardSnapshot::from_fields(&CardSnapshotFields {
                state,
                acct_id: &acct,
                embossed_name: &name,
                expiration_date: expiry,
                status: &status,
            });
            let aid = if pf5 { Aid::Pf5 } else { Aid::Enter };
            if let Ok(Decision::Save(effects)) = decide(aid, &snapshot, &input) {
                let held = standard_card().map_err(|e| TestCaseError::fail(e.to_string()))?;
                let _ = effects.check_change_in_rec(&held);
                if let Err(failure) = effects.rewritten(&held) {
                    prop_assert_eq!(failure, CardUpdateFailure::rewrite_failed());
                }
            }
        }
    }
}
