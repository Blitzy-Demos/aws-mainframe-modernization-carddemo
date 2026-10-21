//! COACTUPC, account update: the rules of endpoints 5 and 6, `GET` and `PUT
//! /api/v1/accounts/{acct_id}` (transaction `CAUP`, map `CACTUPA`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COACTUPC.md`: the account edit
//! (COACTUPC-045 to COACTUPC-047), the read chain (COACTUPC-084 to COACTUPC-099), the shown values
//! and the snapshot (COACTUPC-074 to COACTUPC-077, COACTUPC-100), the receive rule (COACTUPC-007,
//! COACTUPC-009), the change check (COACTUPC-040 to COACTUPC-044), the 24 field edits and the
//! state and ZIP cross edit (COACTUPC-013 to COACTUPC-039, COACTUPC-048 to COACTUPC-067), the
//! decision (COACTUPC-004, COACTUPC-012, COACTUPC-014, COACTUPC-068, COACTUPC-070, COACTUPC-071,
//! COACTUPC-110), the save (COACTUPC-101 to COACTUPC-112) and the changed-by-another-user check
//! (COACTUPC-113 to COACTUPC-116). The `client` rows, F3, F12 and the keys of a finished save
//! among them (COACTUPC-001, COACTUPC-005, COACTUPC-006, COACTUPC-069), send no request.
//!
//! # Flow
//!
//! - Endpoint 5: `api::services::coactupc` edits the path account number with [`account_key`],
//!   reads the cross-reference by account, the account and the customer, and passes the three
//!   [`Lookup`]s to [`fetch`], which answers the read chain's message or builds the
//!   [`AccountUpdateView`] with its [`Snapshot`].
//! - Endpoint 6: the service rebuilds the echoed [`Snapshot`] with [`Snapshot::from_fields`],
//!   re-reads the customer for [`Fetched`] and calls [`decide`]. A [`Decision::Respond`] is the
//!   answer. A [`Decision::Save`] asks for the save of `9600-WRITE-PROCESSING`: the service locks
//!   the account by [`PendingSave::account_key`] and the customer by the cross-reference's
//!   customer ID, runs [`check_change_in_rec`], builds the records with
//!   [`PendingSave::save_effects`], rewrites both, and answers through
//!   [`PendingSave::account_lock_failed`], [`PendingSave::customer_lock_failed`],
//!   [`PendingSave::rewrite_failed`] or [`PendingSave::committed`].
//!
//! # State
//!
//! The COBOL keeps `ACUP-CHANGE-ACTION` and `ACUP-OLD-DETAILS` in the COMMAREA between tasks.
//! The replacement returns both in the [`Snapshot`], which the client echoes and which grants
//! nothing: a save re-reads and compares both records, and every save re-runs the change check
//! and the edits on the fields received (COACTUPC-100, COACTUPC-110). The echoed state still
//! decides what a key does, as `0000-MAIN` and `2000-DECIDE-ACTION` decide it: F5 saves only in
//! state `N`, and ENTER in state `N` runs no edit (COACTUPC-004, COACTUPC-012, COACTUPC-070).
//!
//! # Input and messages
//!
//! Every field enters as the raw text of its map field, at most the map `LENGTH`, including the
//! date, phone and SSN parts and the amounts: the raw-edit-input exception of AAP 0.3.4.1. Each
//! is read as a `MOVE` into its `PIC X(n)` field, and a field that is blank, `*` followed only by
//! spaces, or all low-values is taken as not supplied and holds low-values (`\0`) from then on
//! (COACTUPC-007). Every edit sets its message only while none is set, so the first failing edit
//! in `1200-EDIT-MAP-INPUTS` order is returned with its field (COACTUPC-013); the screen marks
//! that field only (X-COACTUPC-11). No message holds a field value but the account and customer
//! keys the not-found texts quote.
//!
//! # Cardholder data and typed dates
//!
//! The SSN is only ever an [`Ssn`], whose `Debug` and `Serialize` show `***-**-` and its last four
//! digits (COACTUPC-076). The snapshot holds those four digits only, so the change check compares
//! re-entered SSN parts with the stored SSN [`Fetched`] carries, omitted parts mean unchanged, and
//! the save's check compares the last four digits (COACTUPC-044, COACTUPC-064, COACTUPC-115, E32).
//! The cross-reference's card number is in no value this module returns (COACTUPC-090). Stored
//! dates leave typed (COACTUPC-074, COACTUPC-075) and a save stores typed dates (COACTUPC-111,
//! COACTUPC-131).
//!
//! # Halts
//!
//! - `HALTED(H4)`: the CICS `RESP` and `RESP2` positions of the not-found and file-error texts
//!   (COACTUPC-086, COACTUPC-088, COACTUPC-092, COACTUPC-094, COACTUPC-096, COACTUPC-098) hold
//!   spaces here, the initial value of `ERROR-RESP` and `ERROR-RESP2`; no test asserts them.
//! - `HALTED(H5)`: which dates `CEEDAYS` accepts is Language Environment's (COACTUPC-129).
//!   [`edit`] passes CSUTLDTC's stand-in verdict; [`edit_with`] takes the verdict as a parameter.
//! - Halt candidate `X-COACTUPC-H1`: after a failed account read with a found customer the
//!   account block is uninitialised storage; the placeholder of COACTUPC-099 is no parity result.
//! - Halt candidate `X-COACTUPC-H2`: the save keeps the stored `acct_group_id` and both `filler`
//!   fields where the COBOL writes uninitialised storage (COACTUPC-103, COACTUPC-104).
//! - Halt candidate `X-ARITH-5`: an amount with more than two decimal places is stored by
//!   truncation, which no test asserts (COACTUPC-009).
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or an [`UpdateFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input). Digits are read with `char::to_digit` and checked arithmetic.

use core::fmt;
use core::iter::Peekable;
use core::str::Chars;
use std::iter;

use chrono::{Datelike, NaiveDate};
use copybook::edited::EditedPicture;
use copybook::{PicDate, PicU, PicX};

use super::csutldtc::{
    DateMask, EditFailure, LeResult, Part, edit_date_ccyymmdd_with, edit_date_of_birth,
    edit_num_reqd, edit_us_phone, edit_us_ssn, edit_us_state, edit_us_state_zip, validate,
};
use crate::clock::Clock;
use crate::messages::{ABEND_CODE_0001, ABEND_MSG_UNEXPECTED_DATA, AbendData};
use crate::money::{Decimal, Money, MoneyExpr};
use crate::records::account::AccountRecord;
use crate::records::card_xref::CardXrefRecord;
use crate::records::customer::CustomerRecord;
use crate::sensitive::Ssn;

/// `LIT-THISPGM`, the `ABEND-CULPRIT` of the `WHEN OTHER` abend of `2000-DECIDE-ACTION`.
const PROGRAM: &str = "COACTUPC";

/// Width of `WS-RETURN-MSG PIC X(75)`: `STRING` stops at its end (COACTUPC-085).
const MESSAGE_WIDTH: usize = 75;

/// `ERROR-RESP` and `ERROR-RESP2 PIC X(10)` as the replacement fills them: `HALTED(H4)`, the
/// values are CICS-defined, so the positions keep the fields' initial `VALUE SPACES`.
const CICS_CODE: &str = "          ";

/// `PROMPT-FOR-SEARCH-KEYS`, the info line of a screen with no details (COACTUPC-079).
const INFO_SEARCH_KEYS: &str = "Enter or update id of account to update";
/// `PROMPT-FOR-CHANGES`, the info line of states `S` and `E` (COACTUPC-079).
const INFO_CHANGES: &str = "Update account details presented above.";
/// `PROMPT-FOR-CONFIRMATION`, the info line of state `N` (COACTUPC-014, COACTUPC-079).
const INFO_CONFIRMATION: &str = "Changes validated.Press F5 to save";
/// `CONFIRM-UPDATE-SUCCESS`, the info line of state `C` (COACTUPC-079, COACTUPC-108).
const INFO_COMMITTED: &str = "Changes committed to database";
/// `INFORM-FAILURE`, the info line of states `L` and `F` (COACTUPC-079).
const INFO_FAILURE: &str = "Changes unsuccessful. Please try again";

/// `NO-SEARCH-CRITERIA-RECEIVED`, which replaces `Account number not provided` (COACTUPC-045).
const NO_INPUT_RECEIVED: &str = "No input received";
/// The two literals `1210-EDIT-ACCOUNT` strings together (COACTUPC-046).
const ACCOUNT_NOT_VALID: &str = concat!(
    "Account Number if supplied must be a 11 digit",
    " Non-Zero Number"
);
/// `NO-CHANGES-DETECTED` (COACTUPC-043).
const NO_CHANGES: &str = "No change detected with respect to values fetched.";
/// `COULD-NOT-LOCK-ACCT-FOR-UPDATE` (COACTUPC-101).
const LOCK_ACCOUNT_FAILED: &str = "Could not lock account record for update";
/// `COULD-NOT-LOCK-CUST-FOR-UPDATE` (COACTUPC-102).
const LOCK_CUSTOMER_FAILED: &str = "Could not lock customer record for update";
/// `DATA-WAS-CHANGED-BEFORE-UPDATE` (COACTUPC-113).
const RECORD_CHANGED: &str = "Record changed by some one else. Please review";
/// `LOCKED-BUT-UPDATE-FAILED` (COACTUPC-106, COACTUPC-107).
const UPDATE_FAILED: &str = "Update of record failed";

/// `ERROR-OPNAME PIC X(8)` after `MOVE 'READ'` (COACTUPC-087).
const OPNAME_READ: &str = "READ    ";
/// `LIT-CARDXREFNAME-ACCT-PATH` moved into `ERROR-FILE PIC X(9)` (COACTUPC-087).
const FILE_CXACAIX: &str = "CXACAIX  ";
/// `LIT-ACCTFILENAME` moved into `ERROR-FILE PIC X(9)` (COACTUPC-093).
const FILE_ACCTDAT: &str = "ACCTDAT  ";
/// `LIT-CUSTFILENAME` moved into `ERROR-FILE PIC X(9)` (COACTUPC-097).
const FILE_CUSTDAT: &str = "CUSTDAT  ";

/// The names `1200-EDIT-MAP-INPUTS` moves to `WS-EDIT-VARIABLE-NAME` (COACTUPC-015 to
/// COACTUPC-038). `EditFailure::compose` keeps 25 characters, so the cycle-credit name loses its
/// final `t` (COACTUPC-022, X-COACTUPC-13).
const NAME_ACCOUNT_STATUS: &str = "Account Status";
const NAME_OPEN_DATE: &str = "Open Date";
const NAME_CREDIT_LIMIT: &str = "Credit Limit";
const NAME_EXPIRY_DATE: &str = "Expiry Date";
const NAME_CASH_CREDIT_LIMIT: &str = "Cash Credit Limit";
const NAME_REISSUE_DATE: &str = "Reissue Date";
const NAME_CURRENT_BALANCE: &str = "Current Balance";
const NAME_CYCLE_CREDIT: &str = "Current Cycle Credit Limit";
const NAME_CYCLE_DEBIT: &str = "Current Cycle Debit Limit";
const NAME_DATE_OF_BIRTH: &str = "Date of Birth";
const NAME_FICO_SCORE: &str = "FICO Score";
const NAME_FIRST_NAME: &str = "First Name";
const NAME_MIDDLE_NAME: &str = "Middle Name";
const NAME_LAST_NAME: &str = "Last Name";
const NAME_ADDRESS_LINE_1: &str = "Address Line 1";
const NAME_STATE: &str = "State";
const NAME_ZIP: &str = "Zip";
const NAME_CITY: &str = "City";
const NAME_COUNTRY: &str = "Country";
const NAME_PHONE_1: &str = "Phone Number 1";
const NAME_PHONE_2: &str = "Phone Number 2";
const NAME_EFT_ACCOUNT_ID: &str = "EFT Account Id";
const NAME_PRIMARY_CARD_HOLDER: &str = "Primary Card Holder";

/// The literals of the generic edits (COACTUPC-048 to COACTUPC-054, COACTUPC-066).
const LIT_SUPPLIED: &str = " must be supplied.";
const LIT_Y_OR_N: &str = " must be Y or N.";
const LIT_ALPHA_ONLY: &str = " can have alphabets only.";
const LIT_NOT_VALID: &str = " is not valid";
const LIT_FICO_RANGE: &str = ": should be between 300 and 850";

/// `88 FICO-RANGE-IS-VALID VALUES 300 THROUGH 850` (COACTUPC-066).
const FICO_RANGE: core::ops::RangeInclusive<u64> = 300..=850;

/// Field widths of the map `CACTUPA` (`screens.md` COACTUP-013 to COACTUP-120).
const W_ACCOUNT_ID: usize = 11;
const W_FLAG: usize = 1;
const W_AMOUNT: usize = 15;
const W_YEAR: usize = 4;
const W_MONTH_DAY: usize = 2;
const W_DATE: usize = 8;
const W_GROUP_ID: usize = 10;
const W_CUSTOMER_ID: usize = 9;
const W_SSN_1: usize = 3;
const W_SSN_2: usize = 2;
const W_SSN_3: usize = 4;
const W_FICO: usize = 3;
const W_NAME: usize = 25;
const W_ADDRESS: usize = 50;
const W_STATE: usize = 2;
const W_ZIP_SHOWN: usize = 5;
const W_ZIP: usize = 10;
const W_COUNTRY: usize = 3;
const W_PHONE_AREA: usize = 3;
const W_PHONE_PREFIX: usize = 3;
const W_PHONE_LINE: usize = 4;
const W_GOVT_ID: usize = 20;
const W_EFT: usize = 10;

/// The screen state `ACUP-CHANGE-ACTION PIC X(1)` (`COACTUPC.cbl:654-668`), which decides the
/// info line, the keys shown and what the next key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateState {
    /// Spaces or low-values, `ACUP-DETAILS-NOT-FETCHED`: only the account number is enterable.
    NotFetched,
    /// `S`, `ACUP-SHOW-DETAILS`: the fetched details are shown.
    ShowDetails,
    /// `E`, `ACUP-CHANGES-NOT-OK`: an edit failed.
    ChangesNotOk,
    /// `N`, `ACUP-CHANGES-OK-NOT-CONFIRMED`: the changes passed and await F5.
    ChangesOkNotConfirmed,
    /// `C`, `ACUP-CHANGES-OKAYED-AND-DONE`: the save ended, successfully or not (COACTUPC-102).
    ChangesOkayedAndDone,
    /// `L`, `ACUP-CHANGES-OKAYED-LOCK-ERROR`: the account could not be locked.
    LockError,
    /// `F`, `ACUP-CHANGES-OKAYED-BUT-FAILED`: a rewrite failed.
    ChangesFailed,
}

impl UpdateState {
    /// The one-character code the snapshot carries: a space for [`UpdateState::NotFetched`],
    /// otherwise the 88-level's `VALUE`.
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::NotFetched => ' ',
            Self::ShowDetails => 'S',
            Self::ChangesNotOk => 'E',
            Self::ChangesOkNotConfirmed => 'N',
            Self::ChangesOkayedAndDone => 'C',
            Self::LockError => 'L',
            Self::ChangesFailed => 'F',
        }
    }

    /// The state an echoed code names: empty text, spaces or low-values for
    /// [`UpdateState::NotFetched`], one of `S`, `E`, `N`, `C`, `L` and `F` for its state, and
    /// `None` for any other text, a value the program never sets.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "S" => Some(Self::ShowDetails),
            "E" => Some(Self::ChangesNotOk),
            "N" => Some(Self::ChangesOkNotConfirmed),
            "C" => Some(Self::ChangesOkayedAndDone),
            "L" => Some(Self::LockError),
            "F" => Some(Self::ChangesFailed),
            blank if is_blank(blank) => Some(Self::NotFetched),
            _ => None,
        }
    }

    /// The info line `3250-SETUP-INFOMSG` derives from the state alone (COACTUPC-079).
    #[must_use]
    pub const fn info_message(self) -> &'static str {
        match self {
            Self::NotFetched => INFO_SEARCH_KEYS,
            Self::ShowDetails | Self::ChangesNotOk => INFO_CHANGES,
            Self::ChangesOkNotConfirmed => INFO_CONFIRMATION,
            Self::ChangesOkayedAndDone => INFO_COMMITTED,
            Self::LockError | Self::ChangesFailed => INFO_FAILURE,
        }
    }

    /// Whether the `F5=Save` label is shown: in state `N` only (COACTUPC-082).
    #[must_use]
    pub const fn save_key_shown(self) -> bool {
        matches!(self, Self::ChangesOkNotConfirmed)
    }

    /// Whether the `F12=Cancel` label is shown: in states `E`, `N`, `L` and `F` (COACTUPC-082).
    #[must_use]
    pub const fn cancel_key_shown(self) -> bool {
        matches!(
            self,
            Self::ChangesNotOk
                | Self::ChangesOkNotConfirmed
                | Self::LockError
                | Self::ChangesFailed
        )
    }
}

/// The map field a failure names: where the COBOL puts the cursor and the field the screen marks
/// (COACTUPC-081). Each variant is one field of `CACTUPA`; [`UpdateField::map_name`] gives its
/// BMS name, which the request DTO binds to its own field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateField {
    /// `ACCTSID`, the account number.
    AccountId,
    /// `ACSTTUS`, the active status.
    ActiveStatus,
    /// `OPNYEAR`, the open date's year.
    OpenYear,
    /// `OPNMON`, the open date's month.
    OpenMonth,
    /// `OPNDAY`, the open date's day.
    OpenDay,
    /// `ACRDLIM`, the credit limit.
    CreditLimit,
    /// `EXPYEAR`, the expiry date's year.
    ExpiryYear,
    /// `EXPMON`, the expiry date's month.
    ExpiryMonth,
    /// `EXPDAY`, the expiry date's day.
    ExpiryDay,
    /// `ACSHLIM`, the cash credit limit.
    CashCreditLimit,
    /// `RISYEAR`, the reissue date's year.
    ReissueYear,
    /// `RISMON`, the reissue date's month.
    ReissueMonth,
    /// `RISDAY`, the reissue date's day.
    ReissueDay,
    /// `ACURBAL`, the current balance.
    CurrentBalance,
    /// `ACRCYCR`, the current cycle credit.
    CurrentCycleCredit,
    /// `ACRCYDB`, the current cycle debit.
    CurrentCycleDebit,
    /// `ACTSSN1`, the first three SSN digits.
    SsnPart1,
    /// `ACTSSN2`, the fourth and fifth SSN digits.
    SsnPart2,
    /// `ACTSSN3`, the last four SSN digits.
    SsnPart3,
    /// `DOBYEAR`, the date of birth's year.
    DobYear,
    /// `DOBMON`, the date of birth's month.
    DobMonth,
    /// `DOBDAY`, the date of birth's day.
    DobDay,
    /// `ACSTFCO`, the FICO score.
    FicoScore,
    /// `ACSFNAM`, the first name.
    FirstName,
    /// `ACSMNAM`, the middle name.
    MiddleName,
    /// `ACSLNAM`, the last name.
    LastName,
    /// `ACSADL1`, address line 1.
    AddressLine1,
    /// `ACSSTTE`, the state code.
    State,
    /// `ACSZIPC`, the ZIP code.
    Zip,
    /// `ACSCITY`, address line 3.
    City,
    /// `ACSCTRY`, the country code.
    Country,
    /// `ACSPH1A`, phone number 1's area code.
    Phone1Area,
    /// `ACSPH1B`, phone number 1's prefix.
    Phone1Prefix,
    /// `ACSPH1C`, phone number 1's line number.
    Phone1Line,
    /// `ACSPH2A`, phone number 2's area code.
    Phone2Area,
    /// `ACSPH2B`, phone number 2's prefix.
    Phone2Prefix,
    /// `ACSPH2C`, phone number 2's line number.
    Phone2Line,
    /// `ACSEFTC`, the EFT account ID.
    EftAccountId,
    /// `ACSPFLG`, the primary card holder indicator.
    PrimaryCardHolder,
}

impl UpdateField {
    /// The BMS field name in `COACTUP.bms`, such as `ACSTTUS`.
    #[must_use]
    pub const fn map_name(self) -> &'static str {
        match self {
            Self::AccountId => "ACCTSID",
            Self::ActiveStatus => "ACSTTUS",
            Self::OpenYear => "OPNYEAR",
            Self::OpenMonth => "OPNMON",
            Self::OpenDay => "OPNDAY",
            Self::CreditLimit => "ACRDLIM",
            Self::ExpiryYear => "EXPYEAR",
            Self::ExpiryMonth => "EXPMON",
            Self::ExpiryDay => "EXPDAY",
            Self::CashCreditLimit => "ACSHLIM",
            Self::ReissueYear => "RISYEAR",
            Self::ReissueMonth => "RISMON",
            Self::ReissueDay => "RISDAY",
            Self::CurrentBalance => "ACURBAL",
            Self::CurrentCycleCredit => "ACRCYCR",
            Self::CurrentCycleDebit => "ACRCYDB",
            Self::SsnPart1 => "ACTSSN1",
            Self::SsnPart2 => "ACTSSN2",
            Self::SsnPart3 => "ACTSSN3",
            Self::DobYear => "DOBYEAR",
            Self::DobMonth => "DOBMON",
            Self::DobDay => "DOBDAY",
            Self::FicoScore => "ACSTFCO",
            Self::FirstName => "ACSFNAM",
            Self::MiddleName => "ACSMNAM",
            Self::LastName => "ACSLNAM",
            Self::AddressLine1 => "ACSADL1",
            Self::State => "ACSSTTE",
            Self::Zip => "ACSZIPC",
            Self::City => "ACSCITY",
            Self::Country => "ACSCTRY",
            Self::Phone1Area => "ACSPH1A",
            Self::Phone1Prefix => "ACSPH1B",
            Self::Phone1Line => "ACSPH1C",
            Self::Phone2Area => "ACSPH2A",
            Self::Phone2Prefix => "ACSPH2B",
            Self::Phone2Line => "ACSPH2C",
            Self::EftAccountId => "ACSEFTC",
            Self::PrimaryCardHolder => "ACSPFLG",
        }
    }
}

/// The status class of an [`UpdateFailure`], which the API turns into its HTTP status
/// (`modern/audit/README.md`; COACTUPC.md "Reading the table").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit failed or nothing changed: 400.
    Input,
    /// A read found no record: 404 (COACTUPC-085, COACTUPC-091, COACTUPC-095).
    NotFound,
    /// The stored record changed after the fetch: 409 (COACTUPC-113, COACTUPC-114).
    Changed,
    /// A read, lock or rewrite failed: 500 (COACTUPC-087, COACTUPC-101, COACTUPC-102,
    /// COACTUPC-106, COACTUPC-107).
    Store,
    /// The `WHEN OTHER` abend of `2000-DECIDE-ACTION`: 500. The COBOL never reaches it; only an
    /// echoed state code the program never sets does here (see [`decide`]).
    Abend,
}

impl FailureClass {
    /// The HTTP status of the class: 400, 404, 409 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Changed => 409,
            Self::Store | Self::Abend => 500,
        }
    }
}

/// A screen COACTUPC sends instead of the one asked for: the message, the field in error, the
/// status class and the state the screen is left in, which names its info line (COACTUPC-079).
///
/// The message is `WS-RETURN-MSG` without its trailing spaces. It never holds an SSN, a card
/// number or any other field value but the account and customer keys of the not-found texts,
/// so `Debug` and `Display` disclose nothing protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateFailure {
    message: String,
    field: Option<UpdateField>,
    class: FailureClass,
    state: UpdateState,
    shown: Option<Box<ShownAmounts>>,
    details: Option<Box<AccountUpdateView>>,
}

impl UpdateFailure {
    /// A failure with no shown values.
    fn new(
        message: impl Into<String>,
        field: Option<UpdateField>,
        class: FailureClass,
        state: UpdateState,
    ) -> Self {
        Self {
            message: message.into(),
            field,
            class,
            state,
            shown: None,
            details: None,
        }
    }

    /// The failure with the amounts the screen shows beside it (COACTUPC-078).
    fn with_shown(mut self, shown: ShownAmounts) -> Self {
        self.shown = Some(Box::new(shown));
        self
    }

    /// `NO-CHANGES-DETECTED`: 400 on `ACSTTUS`, the echoed state kept (COACTUPC-043).
    fn no_change(state: UpdateState) -> Self {
        Self::new(
            NO_CHANGES,
            Some(UpdateField::ActiveStatus),
            FailureClass::Input,
            state,
        )
    }

    /// The `WHEN OTHER` abend of `2000-DECIDE-ACTION` (`COACTUPC.cbl:2633-2640`): 500 with the
    /// abend message; the screen starts over, as no state survives an abend.
    fn unexpected_data() -> Self {
        Self::new(
            ABEND_MSG_UNEXPECTED_DATA,
            None,
            FailureClass::Abend,
            UpdateState::NotFetched,
        )
    }

    /// `DATA-WAS-CHANGED-BEFORE-UPDATE`: 409, state `S` (COACTUPC-113, COACTUPC-116).
    fn changed() -> Self {
        Self::new(
            RECORD_CHANGED,
            None,
            FailureClass::Changed,
            UpdateState::ShowDetails,
        )
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The field the screen marks and puts the cursor on, if the failure names one.
    #[must_use]
    pub const fn field(&self) -> Option<UpdateField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }

    /// The state the screen is left in, whose info line it shows (COACTUPC-079).
    #[must_use]
    pub const fn state(&self) -> UpdateState {
        self.state
    }

    /// The amounts the screen shows beside the message, when the response shows the values
    /// received: after a failed edit and after a failed save (COACTUPC-078).
    #[must_use]
    pub fn shown_amounts(&self) -> Option<&ShownAmounts> {
        self.shown.as_deref()
    }

    /// The details a failed fetch still shows: only when the account read failed and the
    /// customer read found the customer (COACTUPC-099). Its account block is a placeholder, halt
    /// candidate `X-COACTUPC-H1`.
    #[must_use]
    pub fn details(&self) -> Option<&AccountUpdateView> {
        self.details.as_deref()
    }

    /// The `ABEND-DATA` of the `WHEN OTHER` abend: code `0001`, culprit `COACTUPC`, message
    /// `UNEXPECTED DATA SCENARIO`; `None` for every other failure.
    #[must_use]
    pub fn abend_data(&self) -> Option<AbendData> {
        (self.class == FailureClass::Abend)
            .then(|| AbendData::new(ABEND_CODE_0001, PROGRAM, "", ABEND_MSG_UNEXPECTED_DATA))
    }
}

impl fmt::Display for UpdateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UpdateFailure {}

/// The outcome of one read of the fetch chain, as CICS reports it (COACTUPC-084).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup<T> {
    /// `DFHRESP(NORMAL)`: the record.
    Found(T),
    /// `DFHRESP(NOTFND)`: no record has the key.
    NotFound,
    /// Any other response. For the cross-reference this includes `DUPKEY`, two or more rows for
    /// one account (COACTUPC-089).
    Failed,
    /// The read was not issued. The COBOL issues the account and customer reads whenever the
    /// cross-reference read finds its row, so a link not reached after that is answered as a
    /// failed read.
    NotReached,
}

/// The five amounts as the screen shows them: each the 15-character `+ZZZ,ZZZ,ZZZ.99` string of
/// its value, whose nine integer digits drop a tenth (COACTUPC-074, X-COACTUPC-9), or the text
/// received where its edit failed (COACTUPC-078).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownAmounts {
    /// `ACURBAL`.
    pub current_balance: String,
    /// `ACRDLIM`.
    pub credit_limit: String,
    /// `ACSHLIM`.
    pub cash_credit_limit: String,
    /// `ACRCYCR`.
    pub current_cycle_credit: String,
    /// `ACRCYDB`.
    pub current_cycle_debit: String,
}

/// `value` through `WS-EDIT-CURRENCY-9-2-F PIC +ZZZ,ZZZ,ZZZ.99` (COACTUPC-074).
fn currency(value: Money<10>) -> String {
    EditedPicture::PlusZCommas9Dot99.format(value.as_decimal())
}

/// A phone number as `3202-SHOW-ORIGINAL-VALUES` splits it: characters 2-4, 6-8 and 10-13 of the
/// stored `PIC X(15)`, the digits of its `(AAA)BBB-CCCC` layout (COACTUPC-077).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneDisplay {
    /// `ACSPH1A` or `ACSPH2A`.
    pub area: PicX<3>,
    /// `ACSPH1B` or `ACSPH2B`.
    pub prefix: PicX<3>,
    /// `ACSPH1C` or `ACSPH2C`.
    pub line: PicX<4>,
}

impl PhoneDisplay {
    /// The three parts of a stored phone number, read by array destructuring.
    fn of(phone: &PicX<15>) -> Self {
        let [_, a1, a2, a3, _, b1, b2, b3, _, c1, c2, c3, c4, _, _] = *phone.as_bytes();
        Self {
            area: PicX::from_array([a1, a2, a3]),
            prefix: PicX::from_array([b1, b2, b3]),
            line: PicX::from_array([c1, c2, c3, c4]),
        }
    }
}

/// What endpoint 5 shows in state `S`: the account and customer blocks of
/// `3202-SHOW-ORIGINAL-VALUES` and the [`Snapshot`] the client echoes (COACTUPC-068,
/// COACTUPC-074 to COACTUPC-077, COACTUPC-100).
///
/// Dates are typed, `None` for a blank one; the screen splits them into the map's parts. The SSN
/// is an [`Ssn`], whose `Debug` and `Serialize` show only `***-**-` and its last four digits,
/// which the screen shows in the three SSN fields (COACTUPC-076). No card number is held
/// (COACTUPC-090).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountUpdateView {
    /// `ACCTSID`, read-only once details are shown.
    pub account_id: PicU<11>,
    /// `ACSTTUS`, as stored.
    pub active_status: PicX<1>,
    /// The five amounts as `+ZZZ,ZZZ,ZZZ.99` strings.
    pub amounts: ShownAmounts,
    /// `OPNYEAR`, `OPNMON`, `OPNDAY`.
    pub open_date: Option<NaiveDate>,
    /// `EXPYEAR`, `EXPMON`, `EXPDAY`.
    pub expiry_date: Option<NaiveDate>,
    /// `RISYEAR`, `RISMON`, `RISDAY`.
    pub reissue_date: Option<NaiveDate>,
    /// `AADDGRP`: the stored `ACCT-GROUP-ID`, blank for every fixture account (E07).
    pub group_id: PicX<10>,
    /// `ACSTNUM`, read-only once details are shown.
    pub customer_id: PicU<9>,
    /// `ACTSSN1` to `ACTSSN3`, shown redacted.
    pub ssn: Ssn,
    /// `DOBYEAR`, `DOBMON`, `DOBDAY`.
    pub dob: Option<NaiveDate>,
    /// `ACSTFCO`: three digits.
    pub fico_score: PicU<3>,
    /// `ACSFNAM`.
    pub first_name: PicX<25>,
    /// `ACSMNAM`.
    pub middle_name: PicX<25>,
    /// `ACSLNAM`.
    pub last_name: PicX<25>,
    /// `ACSADL1`.
    pub address_line_1: PicX<50>,
    /// `ACSADL2`.
    pub address_line_2: PicX<50>,
    /// `ACSCITY`: address line 3.
    pub city: PicX<50>,
    /// `ACSSTTE`.
    pub state: PicX<2>,
    /// `ACSZIPC`: the first five characters of the stored ten (COACTUPC-077, X-COACTUPC-9).
    pub zip: PicX<5>,
    /// `ACSCTRY`, read-only once details are shown.
    pub country: PicX<3>,
    /// `ACSPH1A` to `ACSPH1C`.
    pub phone_1: PhoneDisplay,
    /// `ACSPH2A` to `ACSPH2C`.
    pub phone_2: PhoneDisplay,
    /// `ACSGOVT`.
    pub government_id: PicX<20>,
    /// `ACSEFTC`.
    pub eft_account_id: PicX<10>,
    /// `ACSPFLG`.
    pub primary_card_holder: PicX<1>,
    /// The values `9700-CHECK-CHANGE-IN-REC` and `1205-COMPARE-OLD-NEW` compare later, in state
    /// `S`.
    pub snapshot: Snapshot,
}

impl AccountUpdateView {
    /// The state a shown view leaves the screen in (COACTUPC-068).
    pub const STATE: UpdateState = UpdateState::ShowDetails;

    /// The view of a fetched account and its customer (COACTUPC-074 to COACTUPC-077).
    fn new(account: &AccountRecord, customer: &CustomerRecord) -> Self {
        let [z1, z2, z3, z4, z5, ..] = *customer.addr_zip.as_bytes();
        Self {
            account_id: account.id,
            active_status: account.active_status.clone(),
            amounts: ShownAmounts {
                current_balance: currency(account.curr_bal),
                credit_limit: currency(account.credit_limit),
                cash_credit_limit: currency(account.cash_credit_limit),
                current_cycle_credit: currency(account.curr_cyc_credit),
                current_cycle_debit: currency(account.curr_cyc_debit),
            },
            open_date: account.open_date.value(),
            expiry_date: account.expiraion_date.value(),
            reissue_date: account.reissue_date.value(),
            group_id: account.group_id.clone(),
            customer_id: customer.id,
            ssn: customer.ssn,
            dob: customer.dob_yyyy_mm_dd.value(),
            fico_score: customer.fico_credit_score,
            first_name: customer.first_name.clone(),
            middle_name: customer.middle_name.clone(),
            last_name: customer.last_name.clone(),
            address_line_1: customer.addr_line_1.clone(),
            address_line_2: customer.addr_line_2.clone(),
            city: customer.addr_line_3.clone(),
            state: customer.addr_state_cd.clone(),
            zip: PicX::from_array([z1, z2, z3, z4, z5]),
            country: customer.addr_country_cd.clone(),
            phone_1: PhoneDisplay::of(&customer.phone_num_1),
            phone_2: PhoneDisplay::of(&customer.phone_num_2),
            government_id: customer.govt_issued_id.clone(),
            eft_account_id: customer.eft_account_id.clone(),
            primary_card_holder: customer.pri_card_holder_ind.clone(),
            snapshot: Snapshot::from_records(account, customer),
        }
    }

    /// The info line of a shown view: `Update account details presented above.` (COACTUPC-079).
    #[must_use]
    pub const fn info_message(&self) -> &'static str {
        Self::STATE.info_message()
    }
}

/// The values `ACUP-OLD-DETAILS` holds after `9500-STORE-FETCHED-DATA`, and the screen state
/// (COACTUPC-100).
///
/// Amounts are [`Money`] values, the four dates typed, the ZIP and both phone numbers in full as
/// stored, and the SSN only as its last four digits, because the client echoes the snapshot and
/// it must hold no full SSN (E32). A value the echoed text names no value for, such as an amount
/// that does not parse, is held as unknown and equals nothing, so it reads as a change and, at a
/// save, as a record changed by another user. The snapshot grants nothing (X-COACTUPC-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    state: Option<UpdateState>,
    account_id: Option<PicU<11>>,
    active_status: PicX<1>,
    curr_bal: Option<Money<10>>,
    credit_limit: Option<Money<10>>,
    cash_credit_limit: Option<Money<10>>,
    open_date: Option<NaiveDate>,
    expiraion_date: Option<NaiveDate>,
    reissue_date: Option<NaiveDate>,
    curr_cyc_credit: Option<Money<10>>,
    curr_cyc_debit: Option<Money<10>>,
    group_id: PicX<10>,
    customer_id: Option<PicU<9>>,
    first_name: PicX<25>,
    middle_name: PicX<25>,
    last_name: PicX<25>,
    addr_line_1: PicX<50>,
    addr_line_2: PicX<50>,
    addr_line_3: PicX<50>,
    addr_state_cd: PicX<2>,
    addr_country_cd: PicX<3>,
    addr_zip: PicX<10>,
    phone_num_1: PicX<15>,
    phone_num_2: PicX<15>,
    ssn_last4: Option<[u8; 4]>,
    govt_issued_id: PicX<20>,
    dob: Option<NaiveDate>,
    eft_account_id: PicX<10>,
    pri_card_holder_ind: PicX<1>,
    fico_credit_score: Option<PicU<3>>,
}

/// The snapshot as text, the form the client echoes: `S` is `&str` for [`Snapshot::from_fields`]
/// and `String` from [`Snapshot::fields`].
///
/// `state` is the one-character state code ([`UpdateState::code`]); the ID, FICO and SSN fields
/// are their digits; amounts are plain decimal strings such as `-1940.00`; dates are typed;
/// every other field is its stored text. An unknown value is the empty text. For the printable
/// ASCII values every stored record holds, `Snapshot::from_fields(&snapshot.fields())` equals
/// `snapshot`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotFields<S> {
    /// `ACUP-CHANGE-ACTION`.
    pub state: S,
    /// `ACUP-OLD-ACCT-ID`, eleven digits.
    pub account_id: S,
    /// `ACUP-OLD-ACTIVE-STATUS`.
    pub active_status: S,
    /// `ACUP-OLD-CURR-BAL`.
    pub current_balance: S,
    /// `ACUP-OLD-CREDIT-LIMIT`.
    pub credit_limit: S,
    /// `ACUP-OLD-CASH-CREDIT-LIMIT`.
    pub cash_credit_limit: S,
    /// `ACUP-OLD-OPEN-DATE`.
    pub open_date: Option<NaiveDate>,
    /// `ACUP-OLD-EXPIRAION-DATE`.
    pub expiry_date: Option<NaiveDate>,
    /// `ACUP-OLD-REISSUE-DATE`.
    pub reissue_date: Option<NaiveDate>,
    /// `ACUP-OLD-CURR-CYC-CREDIT`.
    pub current_cycle_credit: S,
    /// `ACUP-OLD-CURR-CYC-DEBIT`.
    pub current_cycle_debit: S,
    /// `ACUP-OLD-GROUP-ID`.
    pub group_id: S,
    /// `ACUP-OLD-CUST-ID`, nine digits.
    pub customer_id: S,
    /// `ACUP-OLD-CUST-FIRST-NAME`.
    pub first_name: S,
    /// `ACUP-OLD-CUST-MIDDLE-NAME`.
    pub middle_name: S,
    /// `ACUP-OLD-CUST-LAST-NAME`.
    pub last_name: S,
    /// `ACUP-OLD-CUST-ADDR-LINE-1`.
    pub address_line_1: S,
    /// `ACUP-OLD-CUST-ADDR-LINE-2`.
    pub address_line_2: S,
    /// `ACUP-OLD-CUST-ADDR-LINE-3`.
    pub city: S,
    /// `ACUP-OLD-CUST-ADDR-STATE-CD`.
    pub state_code: S,
    /// `ACUP-OLD-CUST-ADDR-COUNTRY-CD`.
    pub country: S,
    /// `ACUP-OLD-CUST-ADDR-ZIP`, all ten characters.
    pub zip: S,
    /// `ACUP-OLD-CUST-PHONE-NUM-1`, all fifteen characters.
    pub phone_1: S,
    /// `ACUP-OLD-CUST-PHONE-NUM-2`, all fifteen characters.
    pub phone_2: S,
    /// The last four digits of `ACUP-OLD-CUST-SSN`, never the full SSN (E32).
    pub ssn_last4: S,
    /// `ACUP-OLD-CUST-GOVT-ISSUED-ID`.
    pub government_id: S,
    /// `ACUP-OLD-CUST-DOB-YYYY-MM-DD`.
    pub dob: Option<NaiveDate>,
    /// `ACUP-OLD-CUST-EFT-ACCOUNT-ID`.
    pub eft_account_id: S,
    /// `ACUP-OLD-CUST-PRI-HOLDER-IND`.
    pub primary_card_holder: S,
    /// `ACUP-OLD-CUST-FICO-SCORE`, three digits.
    pub fico_score: S,
}

impl Snapshot {
    /// `9500-STORE-FETCHED-DATA` (COACTUPC-100): the fetched account and customer, in state `S`.
    #[must_use]
    pub fn from_records(account: &AccountRecord, customer: &CustomerRecord) -> Self {
        Self {
            state: Some(UpdateState::ShowDetails),
            account_id: Some(account.id),
            active_status: account.active_status.clone(),
            curr_bal: Some(account.curr_bal),
            credit_limit: Some(account.credit_limit),
            cash_credit_limit: Some(account.cash_credit_limit),
            open_date: account.open_date.value(),
            expiraion_date: account.expiraion_date.value(),
            reissue_date: account.reissue_date.value(),
            curr_cyc_credit: Some(account.curr_cyc_credit),
            curr_cyc_debit: Some(account.curr_cyc_debit),
            group_id: account.group_id.clone(),
            customer_id: Some(customer.id),
            first_name: customer.first_name.clone(),
            middle_name: customer.middle_name.clone(),
            last_name: customer.last_name.clone(),
            addr_line_1: customer.addr_line_1.clone(),
            addr_line_2: customer.addr_line_2.clone(),
            addr_line_3: customer.addr_line_3.clone(),
            addr_state_cd: customer.addr_state_cd.clone(),
            addr_country_cd: customer.addr_country_cd.clone(),
            addr_zip: customer.addr_zip.clone(),
            phone_num_1: customer.phone_num_1.clone(),
            phone_num_2: customer.phone_num_2.clone(),
            ssn_last4: Some(customer.ssn.last4()),
            govt_issued_id: customer.govt_issued_id.clone(),
            dob: customer.dob_yyyy_mm_dd.value(),
            eft_account_id: customer.eft_account_id.clone(),
            pri_card_holder_ind: customer.pri_card_holder_ind.clone(),
            fico_credit_score: Some(customer.fico_credit_score),
        }
    }

    /// The snapshot a client echoed (COACTUPC-100). Total: every text yields a snapshot.
    ///
    /// Each text field is read as a `MOVE` into its field, its first bytes kept and the rest
    /// spaces. An ID, the FICO score or the SSN's last four digits that is not exactly its digits,
    /// and an amount that is not a decimal of at most two places within `S9(10)V99`, is held as
    /// unknown; a state code outside [`UpdateState::from_code`] is held as no state.
    #[must_use]
    pub fn from_fields<S: AsRef<str>>(fields: &SnapshotFields<S>) -> Self {
        Self {
            state: UpdateState::from_code(fields.state.as_ref()),
            account_id: PicU::decode(fields.account_id.as_ref().as_bytes()).ok(),
            active_status: moved(fields.active_status.as_ref()),
            curr_bal: plain_amount(fields.current_balance.as_ref()),
            credit_limit: plain_amount(fields.credit_limit.as_ref()),
            cash_credit_limit: plain_amount(fields.cash_credit_limit.as_ref()),
            open_date: fields.open_date,
            expiraion_date: fields.expiry_date,
            reissue_date: fields.reissue_date,
            curr_cyc_credit: plain_amount(fields.current_cycle_credit.as_ref()),
            curr_cyc_debit: plain_amount(fields.current_cycle_debit.as_ref()),
            group_id: moved(fields.group_id.as_ref()),
            customer_id: PicU::decode(fields.customer_id.as_ref().as_bytes()).ok(),
            first_name: moved(fields.first_name.as_ref()),
            middle_name: moved(fields.middle_name.as_ref()),
            last_name: moved(fields.last_name.as_ref()),
            addr_line_1: moved(fields.address_line_1.as_ref()),
            addr_line_2: moved(fields.address_line_2.as_ref()),
            addr_line_3: moved(fields.city.as_ref()),
            addr_state_cd: moved(fields.state_code.as_ref()),
            addr_country_cd: moved(fields.country.as_ref()),
            addr_zip: moved(fields.zip.as_ref()),
            phone_num_1: moved(fields.phone_1.as_ref()),
            phone_num_2: moved(fields.phone_2.as_ref()),
            ssn_last4: four_digits(fields.ssn_last4.as_ref()),
            govt_issued_id: moved(fields.government_id.as_ref()),
            dob: fields.dob,
            eft_account_id: moved(fields.eft_account_id.as_ref()),
            pri_card_holder_ind: moved(fields.primary_card_holder.as_ref()),
            fico_credit_score: PicU::decode(fields.fico_score.as_ref().as_bytes()).ok(),
        }
    }

    /// The snapshot as the text the client echoes, the inverse of [`Snapshot::from_fields`].
    #[must_use]
    pub fn fields(&self) -> SnapshotFields<String> {
        SnapshotFields {
            state: self
                .state
                .map_or_else(|| "?".to_owned(), |s| s.code().to_string()),
            account_id: self.account_id.map_or_else(String::new, digits_of),
            active_status: text_of(self.active_status.as_bytes()),
            current_balance: amount_text(self.curr_bal),
            credit_limit: amount_text(self.credit_limit),
            cash_credit_limit: amount_text(self.cash_credit_limit),
            open_date: self.open_date,
            expiry_date: self.expiraion_date,
            reissue_date: self.reissue_date,
            current_cycle_credit: amount_text(self.curr_cyc_credit),
            current_cycle_debit: amount_text(self.curr_cyc_debit),
            group_id: text_of(self.group_id.as_bytes()),
            customer_id: self.customer_id.map_or_else(String::new, digits_of),
            first_name: text_of(self.first_name.as_bytes()),
            middle_name: text_of(self.middle_name.as_bytes()),
            last_name: text_of(self.last_name.as_bytes()),
            address_line_1: text_of(self.addr_line_1.as_bytes()),
            address_line_2: text_of(self.addr_line_2.as_bytes()),
            city: text_of(self.addr_line_3.as_bytes()),
            state_code: text_of(self.addr_state_cd.as_bytes()),
            country: text_of(self.addr_country_cd.as_bytes()),
            zip: text_of(self.addr_zip.as_bytes()),
            phone_1: text_of(self.phone_num_1.as_bytes()),
            phone_2: text_of(self.phone_num_2.as_bytes()),
            ssn_last4: self.ssn_last4.map_or_else(String::new, |d| text_of(&d)),
            government_id: text_of(self.govt_issued_id.as_bytes()),
            dob: self.dob,
            eft_account_id: text_of(self.eft_account_id.as_bytes()),
            primary_card_holder: text_of(self.pri_card_holder_ind.as_bytes()),
            fico_score: self.fico_credit_score.map_or_else(String::new, digits_of),
        }
    }

    /// The screen state the snapshot was returned with; `None` for an echoed code the program
    /// never sets.
    #[must_use]
    pub const fn state(&self) -> Option<UpdateState> {
        self.state
    }

    /// The same values with the state a response left the screen in, as the client echoes them
    /// next (COACTUPC-100).
    #[must_use]
    pub fn with_state(mut self, state: UpdateState) -> Self {
        self.state = Some(state);
        self
    }
}

/// `text` moved into a `PIC X(N)` field: its first `N` bytes, padded with spaces.
fn moved<const N: usize>(text: &str) -> PicX<N> {
    PicX::moved_from(text.as_bytes())
}

/// `bytes` as text, one character per byte; total over every byte.
fn text_of(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// A `PIC 9(N)` value as its `N` digits, leading zeros included.
fn digits_of<const N: u32>(value: PicU<N>) -> String {
    let mut out = Vec::new();
    value.encode_into(&mut out);
    text_of(&out)
}

/// An amount as a plain decimal string such as `-1940.00`; the empty text when unknown.
fn amount_text(value: Option<Money<10>>) -> String {
    value.map_or_else(String::new, |money| money.as_decimal().to_string())
}

/// A plain decimal string as an `S9(10)V99` amount, `None` when it is not one.
fn plain_amount(text: &str) -> Option<Money<10>> {
    text.parse::<Decimal>()
        .ok()
        .and_then(|value| Money::new(value).ok())
}

/// Exactly four ASCII digits, as the last four digits of an SSN.
fn four_digits(text: &str) -> Option<[u8; 4]> {
    let digits = <[u8; 4]>::try_from(text.as_bytes()).ok()?;
    digits.iter().all(u8::is_ascii_digit).then_some(digits)
}

/// `text` moved into a `PIC X(width)` field: its first `width` characters, padded with spaces.
fn fit(text: &str, width: usize) -> String {
    text.chars().chain(iter::repeat(' ')).take(width).collect()
}

/// `EQUAL SPACES OR EQUAL LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(field: &str) -> bool {
    field.chars().all(|c| c == ' ') || field.chars().all(|c| c == '\0')
}

/// The text a received field is written as: spaces where it was blank (X-COACTUPC-5), and
/// `None` when it holds a character a stored field cannot hold, anything but printable ASCII.
fn written<const N: usize>(field: &str) -> Option<PicX<N>> {
    if field.chars().all(|c| c == '\0') {
        Some(PicX::spaces())
    } else {
        PicX::from_text(field).ok()
    }
}

/// `1210-EDIT-ACCOUNT` with the line of `1200-EDIT-MAP-INPUTS` that follows it (COACTUPC-011,
/// COACTUPC-045 to COACTUPC-047): the raw `ACCTSID` text as the key every read of endpoint 5 uses.
///
/// # Errors
///
/// Status 400 on [`UpdateField::AccountId`], state not fetched:
///
/// - `No input received` when the field is blank, `*` followed by spaces or low-values; the
///   paragraph's own `Account number not provided` is always replaced (COACTUPC-045, X-COACTUPC-2);
/// - `Account Number if supplied must be a 11 digit Non-Zero Number` when it is not eleven digits
///   `0` to `9`, a shorter entry padded with spaces included, or is eleven zeros (COACTUPC-046).
pub fn account_key(acct_id_raw: &str) -> Result<PicU<11>, UpdateFailure> {
    let field = receive(acct_id_raw, W_ACCOUNT_ID);
    let input_error = |message| {
        UpdateFailure::new(
            message,
            Some(UpdateField::AccountId),
            FailureClass::Input,
            UpdateState::NotFetched,
        )
    };
    if is_blank(&field) {
        return Err(input_error(NO_INPUT_RECEIVED));
    }
    match PicU::<11>::decode(field.as_bytes()) {
        Ok(key) if key.get() != 0 => Ok(key),
        _ => Err(input_error(ACCOUNT_NOT_VALID)),
    }
}

/// `9000-READ-ACCT` after a valid account number, then `2000-DECIDE-ACTION` and
/// `3202-SHOW-ORIGINAL-VALUES` (COACTUPC-068, COACTUPC-084 to COACTUPC-100): the answer of
/// endpoint 5.
///
/// `acct_id_raw` is edited by [`account_key`] first. The three lookups are the reads in COBOL
/// order: the cross-reference by account through `CXACAIX`, the account by that key, and the
/// customer by the cross-reference's customer ID. Unless the cross-reference read finds its row
/// the other two are ignored; otherwise both are taken whatever the first returned, because the
/// exits after them test condition names nothing sets (COACTUPC-084). When the account and the
/// customer are found, the result is the [`AccountUpdateView`] in state `S`.
///
/// # Errors
///
/// The first message set stands, except that a customer read failing other than not found
/// replaces it:
///
/// - the account edit of [`account_key`] (400);
/// - no cross-reference row: 404 on `ACCTSID`, `Account:` with the key and ` not found in Cross
///   ref file.  Resp:` and ` Reas:` around the code positions (COACTUPC-085); a failed read,
///   `DUPKEY` included: 500 on `ACCTSID`, the `File Error:` text naming `CXACAIX`
///   (COACTUPC-087, COACTUPC-089). Nothing else is read;
/// - no account row: 404 on `ACCTSID`, `Account:` with the key and ` not found in Acct Master
///   file.Resp:` (COACTUPC-091); a failed account read: 500 on `ACCTSID`, the `File Error:` text
///   naming `ACCTDAT` (COACTUPC-093);
/// - no customer row, when no message is set yet: 404 with no field, `CustId:` with the
///   customer key and ` not found in customer master.Resp: ` (COACTUPC-095); a failed customer
///   read: 500 with the `File Error:` text naming `CUSTDAT`, on `ACCTSID` only when the account
///   read did not succeed (COACTUPC-097).
///
/// Every failure leaves the state not fetched, except one: a failed account read with a found
/// customer leaves state `S` and carries [`UpdateFailure::details`] (COACTUPC-099). The code
/// positions of these texts are `HALTED(H4)` (COACTUPC-086 to COACTUPC-098).
pub fn fetch(
    acct_id_raw: &str,
    xref: Lookup<&CardXrefRecord>,
    account: Lookup<&AccountRecord>,
    customer: Lookup<&CustomerRecord>,
) -> Result<AccountUpdateView, UpdateFailure> {
    let key = account_key(acct_id_raw)?;
    let cust_key = match xref {
        Lookup::Found(row) => row.cust_id,
        Lookup::NotFound => return Err(xref_not_found(key)),
        Lookup::Failed | Lookup::NotReached => return Err(file_error(FILE_CXACAIX, true)),
    };
    let account_failure = match account {
        Lookup::Found(_) => None,
        Lookup::NotFound => Some(account_not_found(key)),
        Lookup::Failed | Lookup::NotReached => Some(file_error(FILE_ACCTDAT, true)),
    };
    match (account, customer) {
        (Lookup::Found(stored), Lookup::Found(cust)) => Ok(AccountUpdateView::new(stored, cust)),
        (_, Lookup::Found(cust)) => Err(unread_account(key, cust, account_failure)),
        (_, Lookup::NotFound) => {
            Err(account_failure.unwrap_or_else(|| customer_not_found(cust_key)))
        }
        (_, Lookup::Failed | Lookup::NotReached) => {
            Err(file_error(FILE_CUSTDAT, account_failure.is_some()))
        }
    }
}

/// COACTUPC-099: the account read failed and the customer read found the customer. The state
/// becomes `S`, with the account read's message, status and field.
///
/// Halt candidate `X-COACTUPC-H1`: the COBOL shows and keeps an account block moved from
/// `ACCOUNT-RECORD`, which no read filled in the task, so its values have no single result. The
/// placeholder here, the key with blank and zero fields, is the replacement's and no parity
/// result; the customer block is exact.
fn unread_account(
    key: PicU<11>,
    customer: &CustomerRecord,
    account_failure: Option<UpdateFailure>,
) -> UpdateFailure {
    let placeholder = AccountRecord {
        id: key,
        active_status: PicX::spaces(),
        curr_bal: Money::ZERO,
        credit_limit: Money::ZERO,
        cash_credit_limit: Money::ZERO,
        open_date: PicDate::BLANK,
        expiraion_date: PicDate::BLANK,
        reissue_date: PicDate::BLANK,
        curr_cyc_credit: Money::ZERO,
        curr_cyc_debit: Money::ZERO,
        addr_zip: PicX::spaces(),
        group_id: PicX::spaces(),
        filler: PicX::spaces(),
    };
    let mut failure = account_failure.unwrap_or_else(|| file_error(FILE_ACCTDAT, true));
    failure.state = UpdateState::ShowDetails;
    failure.details = Some(Box::new(AccountUpdateView::new(&placeholder, customer)));
    failure
}

/// The pieces of a `STRING ... DELIMITED BY SIZE INTO WS-RETURN-MSG`: cut at the 75-byte field,
/// without its trailing spaces.
fn strung(pieces: &[&str]) -> String {
    let message: String = pieces
        .iter()
        .flat_map(|piece| piece.chars())
        .take(MESSAGE_WIDTH)
        .collect();
    message.trim_end_matches(' ').to_owned()
}

/// `9200-GETCARDXREF-BYACCT`, `NOTFND` (COACTUPC-085); code positions `HALTED(H4)`.
fn xref_not_found(key: PicU<11>) -> UpdateFailure {
    let key = digits_of(key);
    let pieces = [
        "Account:",
        key.as_str(),
        " not found in",
        " Cross ref file.  Resp:",
        CICS_CODE,
        " Reas:",
        CICS_CODE,
    ];
    UpdateFailure::new(
        strung(&pieces),
        Some(UpdateField::AccountId),
        FailureClass::NotFound,
        UpdateState::NotFetched,
    )
}

/// `9300-GETACCTDATA-BYACCT`, `NOTFND` (COACTUPC-091); code positions `HALTED(H4)`.
fn account_not_found(key: PicU<11>) -> UpdateFailure {
    let key = digits_of(key);
    let pieces = [
        "Account:",
        key.as_str(),
        " not found in",
        " Acct Master file.Resp:",
        CICS_CODE,
        " Reas:",
        CICS_CODE,
    ];
    UpdateFailure::new(
        strung(&pieces),
        Some(UpdateField::AccountId),
        FailureClass::NotFound,
        UpdateState::NotFetched,
    )
}

/// `9400-GETCUSTDATA-BYCUST`, `NOTFND` (COACTUPC-095): no field; code positions `HALTED(H4)`.
fn customer_not_found(key: PicU<9>) -> UpdateFailure {
    let key = digits_of(key);
    let pieces = [
        "CustId:",
        key.as_str(),
        " not found",
        " in customer master.Resp: ",
        CICS_CODE,
        " REAS:",
        CICS_CODE,
    ];
    UpdateFailure::new(
        strung(&pieces),
        None,
        FailureClass::NotFound,
        UpdateState::NotFetched,
    )
}

/// `WS-FILE-ERROR-MESSAGE` after a failed `READ` of `file`, its first 75 of 80 bytes
/// (COACTUPC-087, COACTUPC-093, COACTUPC-097); code positions `HALTED(H4)`.
fn file_error(file: &str, on_account_field: bool) -> UpdateFailure {
    let pieces = [
        "File Error: ",
        OPNAME_READ,
        " on ",
        file,
        " returned RESP ",
        CICS_CODE,
        ",RESP2 ",
        CICS_CODE,
        "     ",
    ];
    UpdateFailure::new(
        strung(&pieces),
        on_account_field.then_some(UpdateField::AccountId),
        FailureClass::Store,
        UpdateState::NotFetched,
    )
}

/// The key of endpoint 6 (`AccountUpdateRequest.aid`). The DTO maps every other key to
/// [`Aid::Enter`], as `0000-MAIN` processes an invalid key as ENTER (COACTUPC-003, COACTUPC-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Aid {
    /// ENTER: the change check and the edits.
    Enter,
    /// F5: the save, valid only in state `N`.
    Pf5,
}

/// A date's three map fields: year (4), month (2) and day (2), each raw text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DateParts<'a> {
    /// `OPNYEAR`, `EXPYEAR`, `RISYEAR` or `DOBYEAR`.
    pub year: &'a str,
    /// The month field.
    pub month: &'a str,
    /// The day field.
    pub day: &'a str,
}

/// A phone number's three map fields: area code (3), prefix (3) and line number (4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhoneParts<'a> {
    /// `ACSPH1A` or `ACSPH2A`.
    pub area: &'a str,
    /// `ACSPH1B` or `ACSPH2B`.
    pub prefix: &'a str,
    /// `ACSPH1C` or `ACSPH2C`.
    pub line: &'a str,
}

/// The three SSN map fields as re-entered: `ACTSSN1` (3), `ACTSSN2` (2), `ACTSSN3` (4). A part
/// the request omits is the empty text, which reads as blank (COACTUPC-064).
///
/// `Debug` prints nothing of the parts: they are cardholder data.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct SsnParts<'a> {
    /// The first three digits.
    pub part1: &'a str,
    /// The fourth and fifth digits.
    pub part2: &'a str,
    /// The last four digits.
    pub part3: &'a str,
}

impl fmt::Debug for SsnParts<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SsnParts(***)")
    }
}

/// The fields of an endpoint 6 request, each the raw text of its map field (COACTUPC-007).
///
/// `account_id` is the path's account number, which stands for the read-only `ACCTSID` field.
/// `ssn` is `None` when the request omits all three SSN parts, which means unchanged (E32,
/// COACTUPC-064). An absent field is the empty text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountUpdateInput<'a> {
    /// `ACCTSID` (11).
    pub account_id: &'a str,
    /// `ACSTTUS` (1).
    pub active_status: &'a str,
    /// `OPNYEAR`, `OPNMON`, `OPNDAY`.
    pub open_date: DateParts<'a>,
    /// `ACRDLIM` (15).
    pub credit_limit: &'a str,
    /// `EXPYEAR`, `EXPMON`, `EXPDAY`.
    pub expiry_date: DateParts<'a>,
    /// `ACSHLIM` (15).
    pub cash_credit_limit: &'a str,
    /// `RISYEAR`, `RISMON`, `RISDAY`.
    pub reissue_date: DateParts<'a>,
    /// `ACURBAL` (15).
    pub current_balance: &'a str,
    /// `ACRCYCR` (15).
    pub current_cycle_credit: &'a str,
    /// `AADDGRP` (10).
    pub group_id: &'a str,
    /// `ACRCYDB` (15).
    pub current_cycle_debit: &'a str,
    /// `ACSTNUM` (9).
    pub customer_id: &'a str,
    /// `ACTSSN1` to `ACTSSN3`, or `None` when all three are omitted.
    pub ssn: Option<SsnParts<'a>>,
    /// `DOBYEAR`, `DOBMON`, `DOBDAY`.
    pub dob: DateParts<'a>,
    /// `ACSTFCO` (3).
    pub fico_score: &'a str,
    /// `ACSFNAM` (25).
    pub first_name: &'a str,
    /// `ACSMNAM` (25).
    pub middle_name: &'a str,
    /// `ACSLNAM` (25).
    pub last_name: &'a str,
    /// `ACSADL1` (50).
    pub address_line_1: &'a str,
    /// `ACSSTTE` (2).
    pub state: &'a str,
    /// `ACSADL2` (50).
    pub address_line_2: &'a str,
    /// `ACSZIPC` (5).
    pub zip: &'a str,
    /// `ACSCITY` (50), address line 3.
    pub city: &'a str,
    /// `ACSCTRY` (3).
    pub country: &'a str,
    /// `ACSPH1A`, `ACSPH1B`, `ACSPH1C`.
    pub phone_1: PhoneParts<'a>,
    /// `ACSGOVT` (20).
    pub government_id: &'a str,
    /// `ACSPH2A`, `ACSPH2B`, `ACSPH2C`.
    pub phone_2: PhoneParts<'a>,
    /// `ACSEFTC` (10).
    pub eft_account_id: &'a str,
    /// `ACSPFLG` (1).
    pub primary_card_holder: &'a str,
}

/// What the service read for this request that the COBOL holds from its own screen: the stored
/// SSN of the account's customer (E32).
///
/// The COBOL receives back the full SSN it showed. The replacement never shows it, so omitted
/// SSN parts are edited as the stored SSN's parts and re-entered parts are compared with it
/// (COACTUPC-044, COACTUPC-064). `Debug` shows the SSN redacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fetched {
    ssn: Ssn,
}

impl Fetched {
    /// The context of a customer the service has just read.
    #[must_use]
    pub const fn new(customer: &CustomerRecord) -> Self {
        Self { ssn: customer.ssn }
    }
}

/// `1100-RECEIVE-MAP` for one field of `width` characters: its text moved into the field, or
/// low-values when the field is blank, `*` followed only by spaces or all low-values
/// (COACTUPC-007).
fn receive(text: &str, width: usize) -> String {
    let field = fit(text, width);
    let erased = is_blank(&field)
        || field
            .strip_prefix('*')
            .is_some_and(|rest| rest.chars().all(|c| c == ' '));
    if erased {
        iter::repeat_n('\0', width).collect()
    } else {
        field
    }
}

/// A received amount: its `PIC X(15)` text and what `TEST-NUMVAL-C` and `NUMVAL-C` made of it
/// (COACTUPC-009).
#[derive(Debug, Clone)]
struct Amount {
    text: String,
    value: AmountValue,
}

/// The `S9(10)V99` an amount was stored into by `1100-RECEIVE-MAP`.
#[derive(Debug, Clone, Copy)]
enum AmountValue {
    /// Blank: low-values, which equal no stored amount.
    Blank,
    /// Text `TEST-NUMVAL-C` rejects: the field keeps the spaces of `INITIALIZE`, which equal no
    /// stored amount.
    Rejected,
    /// The `NUMVAL-C` value stored into `S9(10)V99`.
    Value(Money<10>),
}

impl Amount {
    /// The received amount of `text`.
    fn receive(text: &str) -> Self {
        let field = receive(text, W_AMOUNT);
        let value = if is_blank(&field) {
            AmountValue::Blank
        } else {
            // X-ARITH-5: text with more than two decimal places has no single COBOL value; the
            // store truncates it, which no test asserts.
            numval_c(&field).map_or(AmountValue::Rejected, |exact| {
                AmountValue::Value(MoneyExpr::from_decimal(exact).store::<10>())
            })
        };
        Self { text: field, value }
    }

    /// The amount as `3203-SHOW-UPDATED-VALUES` shows it: its value through `+ZZZ,ZZZ,ZZZ.99`,
    /// or the text received (COACTUPC-078).
    fn shown(&self) -> String {
        match self.value {
            AmountValue::Value(money) => currency(money),
            AmountValue::Blank | AmountValue::Rejected => self.text.replace('\0', " "),
        }
    }

    /// Whether the received value equals a fetched one: only a value does (COACTUPC-040).
    fn equals(&self, old: Option<Money<10>>) -> bool {
        match (self.value, old) {
            (AmountValue::Value(new), Some(old)) => new == old,
            _ => false,
        }
    }
}

/// `FUNCTION NUMVAL-C` of `text` when `FUNCTION TEST-NUMVAL-C` accepts it, `None` when it does
/// not (COACTUPC-009, COACTUPC-054). The value is exact: no floating point (X-ARITH-1).
///
/// The accepted form, which the reviewer confirms against IBM's `NUMVAL-C` rules: optional
/// spaces; an optional leading `+` or `-` with optional spaces; an optional currency sign `$`
/// with optional spaces; a number, digits with commas among the integer digits, each comma
/// between two integer digits, and at most one decimal point, or a point followed by digits;
/// optional spaces; and, only without a leading sign, an optional trailing `+`, `-`, `CR` or `DB`
/// in either case, with optional spaces. `1,234.56`, `1,23,4`, `$1,234.56`, `-50`, `50-`, `50CR`
/// and the `+ZZZ,ZZZ,ZZZ.99` strings endpoint 5 shows pass; `1.2.3`, `12 34`, `1,`, `1,,2`,
/// `1,.00` and letters fail.
fn numval_c(text: &str) -> Option<Decimal> {
    let mut chars = text.chars().peekable();
    skip_spaces(&mut chars);
    let leading = take_sign(&mut chars);
    if leading.is_some() {
        skip_spaces(&mut chars);
    }
    if chars.next_if_eq(&'$').is_some() {
        skip_spaces(&mut chars);
    }
    let (mantissa, scale) = take_number(&mut chars)?;
    skip_spaces(&mut chars);
    let trailing = match leading {
        Some(_) => None,
        None => match take_trailing_sign(&mut chars) {
            Trailing::Absent => None,
            Trailing::Present(sign) => Some(sign),
            Trailing::Incomplete => return None,
        },
    };
    skip_spaces(&mut chars);
    if chars.next().is_some() {
        return None;
    }
    let magnitude = Decimal::try_from_i128_with_scale(mantissa, scale).ok()?;
    let negative = matches!(leading.or(trailing), Some(Sign::Minus));
    Some(if negative { -magnitude } else { magnitude })
}

/// The sign of a `NUMVAL-C` string.
#[derive(Clone, Copy)]
enum Sign {
    /// `+`.
    Plus,
    /// `-`, `CR` or `DB`.
    Minus,
}

/// Skips a run of spaces.
fn skip_spaces(chars: &mut Peekable<Chars<'_>>) {
    while chars.next_if_eq(&' ').is_some() {}
}

/// A leading `+` or `-`.
fn take_sign(chars: &mut Peekable<Chars<'_>>) -> Option<Sign> {
    match chars.next_if(|c| matches!(c, '+' | '-'))? {
        '-' => Some(Sign::Minus),
        _ => Some(Sign::Plus),
    }
}

/// What follows the number of a `NUMVAL-C` string where a trailing sign may stand.
enum Trailing {
    /// No trailing sign.
    Absent,
    /// `+`, `-`, `CR` or `DB`.
    Present(Sign),
    /// A `C` or `D` not completed to `CR` or `DB`.
    Incomplete,
}

/// An optional trailing sign, in either letter case for `CR` and `DB`.
fn take_trailing_sign(chars: &mut Peekable<Chars<'_>>) -> Trailing {
    if let Some(sign) = take_sign(chars) {
        return Trailing::Present(sign);
    }
    let Some(first) = chars.next_if(|c| matches!(c, 'C' | 'c' | 'D' | 'd')) else {
        return Trailing::Absent;
    };
    let second = if first.eq_ignore_ascii_case(&'C') {
        'R'
    } else {
        'B'
    };
    if chars.next_if(|c| c.eq_ignore_ascii_case(&second)).is_some() {
        Trailing::Present(Sign::Minus)
    } else {
        Trailing::Incomplete
    }
}

/// The number of a `NUMVAL-C` string, `digit[,digit]...[.[digit]]` or `.digit` where each
/// `digit` is a run of one or more digits: the value of its digits with the commas dropped, and
/// the count of digits after its point. Groups need not have three digits. `None` when the text
/// has no digit, a second point, or a comma that does not stand between two integer digits: a
/// comma first, after the point, before the point, beside another comma, or last.
fn take_number(chars: &mut Peekable<Chars<'_>>) -> Option<(i128, u32)> {
    let mut mantissa = 0_i128;
    let mut digits = 0_u32;
    let mut scale = 0_u32;
    let mut seen_point = false;
    let mut comma_pending = false;
    while let Some(c) = chars.next_if(|c| c.is_ascii_digit() || matches!(c, ',' | '.')) {
        match c {
            '.' if seen_point || comma_pending => return None,
            '.' => seen_point = true,
            ',' if seen_point || comma_pending || digits == 0 => return None,
            ',' => comma_pending = true,
            _ => {
                let digit = c.to_digit(10)?;
                mantissa = mantissa.checked_mul(10)?.checked_add(i128::from(digit))?;
                digits = digits.checked_add(1)?;
                comma_pending = false;
                if seen_point {
                    scale = scale.checked_add(1)?;
                }
            }
        }
    }
    (digits > 0 && !comma_pending).then_some((mantissa, scale))
}

/// A received date: the year, month and day fields as `1100-RECEIVE-MAP` leaves them.
#[derive(Debug, Clone)]
struct DateIn {
    year: String,
    month: String,
    day: String,
}

impl DateIn {
    /// The received parts of `parts`.
    fn receive(parts: DateParts<'_>) -> Self {
        Self {
            year: receive(parts.year, W_YEAR),
            month: receive(parts.month, W_MONTH_DAY),
            day: receive(parts.day, W_MONTH_DAY),
        }
    }

    /// The eight characters of the `PIC X(08)` date group.
    fn text(&self) -> String {
        [self.year.as_str(), &self.month, &self.day].concat()
    }
}

/// A received phone number: area code, prefix and line number.
#[derive(Debug, Clone)]
struct PhoneIn {
    area: String,
    prefix: String,
    line: String,
}

impl PhoneIn {
    /// The received parts of `parts`.
    fn receive(parts: PhoneParts<'_>) -> Self {
        Self {
            area: receive(parts.area, W_PHONE_AREA),
            prefix: receive(parts.prefix, W_PHONE_PREFIX),
            line: receive(parts.line, W_PHONE_LINE),
        }
    }

    /// Whether each part equals the stored phone number's characters 2-4, 6-8 and 10-13, exactly
    /// (COACTUPC-041).
    fn equals(&self, old: &PicX<15>) -> bool {
        let shown = PhoneDisplay::of(old);
        self.area == text_of(shown.area.as_bytes())
            && self.prefix == text_of(shown.prefix.as_bytes())
            && self.line == text_of(shown.line.as_bytes())
    }

    /// `STRING '(' A ')' B '-' C DELIMITED BY SIZE` into the 15-byte field `INITIALIZE` cleared:
    /// thirteen characters and two spaces (COACTUPC-104), blank parts as spaces (X-COACTUPC-5).
    fn written(&self) -> Option<PicX<15>> {
        let part = |field: &str| field.replace('\0', " ");
        let text = format!(
            "({}){}-{}  ",
            part(&self.area),
            part(&self.prefix),
            part(&self.line)
        );
        PicX::from_text(&text).ok()
    }
}

/// Re-entered SSN parts after `1100-RECEIVE-MAP`. `Debug` prints nothing of them.
#[derive(Clone)]
struct SsnText([String; 3]);

impl fmt::Debug for SsnText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SsnText(***)")
    }
}

/// Every field of a request after `1100-RECEIVE-MAP` (COACTUPC-007, COACTUPC-009): `ACUP-NEW-*`
/// and the five `ACUP-NEW-*-X` amount texts.
#[derive(Debug, Clone)]
struct Received {
    account_id: String,
    active_status: String,
    credit_limit: Amount,
    cash_credit_limit: Amount,
    curr_bal: Amount,
    curr_cyc_credit: Amount,
    curr_cyc_debit: Amount,
    open: DateIn,
    expiry: DateIn,
    reissue: DateIn,
    group_id: String,
    customer_id: String,
    ssn: Option<SsnText>,
    dob: DateIn,
    fico: String,
    first_name: String,
    middle_name: String,
    last_name: String,
    address_line_1: String,
    address_line_2: String,
    city: String,
    state: String,
    country: String,
    zip: String,
    phone_1: PhoneIn,
    phone_2: PhoneIn,
    government_id: String,
    eft_account_id: String,
    primary_card_holder: String,
}

impl Received {
    /// `1100-RECEIVE-MAP` over `input`.
    ///
    /// The 5-character ZIP is moved into the 10-character `ACUP-NEW-CUST-ADDR-ZIP`, so a ZIP
    /// that is not blank is followed by five spaces (COACTUPC-032).
    fn new(input: &AccountUpdateInput<'_>) -> Self {
        let zip = receive(input.zip, W_ZIP_SHOWN);
        let zip = if is_blank(&zip) {
            receive("", W_ZIP)
        } else {
            fit(&zip, W_ZIP)
        };
        Self {
            account_id: receive(input.account_id, W_ACCOUNT_ID),
            active_status: receive(input.active_status, W_FLAG),
            credit_limit: Amount::receive(input.credit_limit),
            cash_credit_limit: Amount::receive(input.cash_credit_limit),
            curr_bal: Amount::receive(input.current_balance),
            curr_cyc_credit: Amount::receive(input.current_cycle_credit),
            curr_cyc_debit: Amount::receive(input.current_cycle_debit),
            open: DateIn::receive(input.open_date),
            expiry: DateIn::receive(input.expiry_date),
            reissue: DateIn::receive(input.reissue_date),
            group_id: receive(input.group_id, W_GROUP_ID),
            customer_id: receive(input.customer_id, W_CUSTOMER_ID),
            ssn: input.ssn.map(|parts| {
                SsnText([
                    receive(parts.part1, W_SSN_1),
                    receive(parts.part2, W_SSN_2),
                    receive(parts.part3, W_SSN_3),
                ])
            }),
            dob: DateIn::receive(input.dob),
            fico: receive(input.fico_score, W_FICO),
            first_name: receive(input.first_name, W_NAME),
            middle_name: receive(input.middle_name, W_NAME),
            last_name: receive(input.last_name, W_NAME),
            address_line_1: receive(input.address_line_1, W_ADDRESS),
            address_line_2: receive(input.address_line_2, W_ADDRESS),
            city: receive(input.city, W_ADDRESS),
            state: receive(input.state, W_STATE),
            country: receive(input.country, W_COUNTRY),
            zip,
            phone_1: PhoneIn::receive(input.phone_1),
            phone_2: PhoneIn::receive(input.phone_2),
            government_id: receive(input.government_id, W_GOVT_ID),
            eft_account_id: receive(input.eft_account_id, W_EFT),
            primary_card_holder: receive(input.primary_card_holder, W_FLAG),
        }
    }

    /// The five amounts as the screen shows them after this request (COACTUPC-078).
    fn shown_amounts(&self) -> ShownAmounts {
        ShownAmounts {
            current_balance: self.curr_bal.shown(),
            credit_limit: self.credit_limit.shown(),
            cash_credit_limit: self.cash_credit_limit.shown(),
            current_cycle_credit: self.curr_cyc_credit.shown(),
            current_cycle_debit: self.curr_cyc_debit.shown(),
        }
    }
}

/// What `1205-COMPARE-OLD-NEW` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeCheck {
    /// `NO-CHANGES-FOUND`: every term equal (COACTUPC-043).
    Unchanged,
    /// `CHANGE-HAS-OCCURRED`: a term differs.
    Changed,
}

/// `1205-COMPARE-OLD-NEW` (COACTUPC-040 to COACTUPC-044): the received fields of `input` against
/// the fetched values of `snapshot`.
///
/// The account terms are compared first: the account ID and the three dates exactly, the status
/// after `UPPER-CASE`, the amounts by value and the group ID after `TRIM` and `UPPER-CASE`. Then
/// the customer terms: after `TRIM` and `UPPER-CASE` the customer ID, names, address lines, state,
/// country, ZIP, government ID and primary-holder indicator; exactly the six phone parts, the
/// date of birth, the EFT account ID and the FICO score. A blank field holds low-values, which
/// `TRIM` keeps and no stored value equals, so it always counts as a change (COACTUPC-042), and a
/// stored ZIP+4 never equals the five characters received (X-COACTUPC-9).
///
/// The SSN term follows E32: omitted parts are equal; re-entered parts are equal when their nine
/// characters equal the stored SSN in `fetched` (COACTUPC-044).
#[must_use]
pub fn compare_old_new(
    input: &AccountUpdateInput<'_>,
    snapshot: &Snapshot,
    fetched: &Fetched,
) -> ChangeCheck {
    compare_received(&Received::new(input), snapshot, *fetched)
}

/// [`compare_old_new`] over received fields.
fn compare_received(new: &Received, old: &Snapshot, fetched: Fetched) -> ChangeCheck {
    if account_terms_equal(new, old) && customer_terms_equal(new, old, fetched) {
        ChangeCheck::Unchanged
    } else {
        ChangeCheck::Changed
    }
}

/// The account terms of `1205-COMPARE-OLD-NEW` (COACTUPC-040).
fn account_terms_equal(new: &Received, old: &Snapshot) -> bool {
    old.account_id
        .is_some_and(|id| new.account_id == digits_of(id))
        && new
            .active_status
            .eq_ignore_ascii_case(&text_of(old.active_status.as_bytes()))
        && new.curr_bal.equals(old.curr_bal)
        && new.credit_limit.equals(old.credit_limit)
        && new.cash_credit_limit.equals(old.cash_credit_limit)
        && date_equals(&new.open, old.open_date)
        && date_equals(&new.expiry, old.expiraion_date)
        && date_equals(&new.reissue, old.reissue_date)
        && new.curr_cyc_credit.equals(old.curr_cyc_credit)
        && new.curr_cyc_debit.equals(old.curr_cyc_debit)
        && trimmed_equals(&new.group_id, old.group_id.as_bytes())
}

/// The customer terms of `1205-COMPARE-OLD-NEW` (COACTUPC-041, COACTUPC-044).
fn customer_terms_equal(new: &Received, old: &Snapshot, fetched: Fetched) -> bool {
    old.customer_id
        .is_some_and(|id| trimmed_equals(&new.customer_id, digits_of(id).as_bytes()))
        && trimmed_equals(&new.first_name, old.first_name.as_bytes())
        && trimmed_equals(&new.middle_name, old.middle_name.as_bytes())
        && trimmed_equals(&new.last_name, old.last_name.as_bytes())
        && trimmed_equals(&new.address_line_1, old.addr_line_1.as_bytes())
        && trimmed_equals(&new.address_line_2, old.addr_line_2.as_bytes())
        && trimmed_equals(&new.city, old.addr_line_3.as_bytes())
        && trimmed_equals(&new.state, old.addr_state_cd.as_bytes())
        && trimmed_equals(&new.country, old.addr_country_cd.as_bytes())
        && trimmed_equals(&new.zip, old.addr_zip.as_bytes())
        && new.phone_1.equals(&old.phone_num_1)
        && new.phone_2.equals(&old.phone_num_2)
        && ssn_equals(new.ssn.as_ref(), fetched.ssn)
        && trimmed_equals(&new.government_id, old.govt_issued_id.as_bytes())
        && date_equals(&new.dob, old.dob)
        && new.eft_account_id == text_of(old.eft_account_id.as_bytes())
        && trimmed_equals(&new.primary_card_holder, old.pri_card_holder_ind.as_bytes())
        && old
            .fico_credit_score
            .is_some_and(|score| new.fico == digits_of(score))
}

/// `FUNCTION UPPER-CASE(FUNCTION TRIM(new)) = FUNCTION UPPER-CASE(FUNCTION TRIM(old))`, the
/// shorter operand padded with spaces: `TRIM` drops spaces only, never low-values, and
/// `UPPER-CASE` changes `a` to `z` only.
fn trimmed_equals(new: &str, old: &[u8]) -> bool {
    new.trim_matches(' ')
        .eq_ignore_ascii_case(text_of(old).trim_matches(' '))
}

/// A received date group against a fetched date: the eight characters against the date's
/// `YYYYMMDD`, or eight spaces for a blank one (COACTUPC-040).
fn date_equals(new: &DateIn, old: Option<NaiveDate>) -> bool {
    let old_text = match old {
        None => Some(fit("", W_DATE)),
        Some(date) => (0..=9999)
            .contains(&date.year())
            .then(|| format!("{:04}{:02}{:02}", date.year(), date.month(), date.day())),
    };
    old_text.is_some_and(|text| new.text() == text)
}

/// The SSN term under E32 (COACTUPC-044): omitted parts are equal; re-entered parts are equal
/// when their nine characters are the stored SSN's digits.
fn ssn_equals(new: Option<&SsnText>, stored: Ssn) -> bool {
    new.is_none_or(|SsnText(parts)| parts.concat() == digits_of(*stored.expose()))
}

/// A failed field edit: the message it sets and the field it names.
struct FieldError {
    message: String,
    field: UpdateField,
}

impl FieldError {
    /// `TRIM(name)` followed by `literal`, on `field` (COACTUPC-048 to COACTUPC-054).
    fn compose(name: &str, literal: &str, field: UpdateField) -> Self {
        Self {
            message: EditFailure::compose(name, literal, Part::Field)
                .message()
                .to_owned(),
            field,
        }
    }

    /// A CSUTLDTC edit's failure on the field `field_of` names for its part.
    fn from_edit(failure: &EditFailure, field_of: impl FnOnce(Part) -> UpdateField) -> Self {
        Self {
            message: failure.message().to_owned(),
            field: field_of(failure.part()),
        }
    }
}

/// The three map fields of one date.
#[derive(Clone, Copy)]
struct DateFields {
    year: UpdateField,
    month: UpdateField,
    day: UpdateField,
}

impl DateFields {
    /// The field an edit's part names; the year for a whole-date check (COACTUPC-117).
    const fn of(self, part: Part) -> UpdateField {
        match part {
            Part::Month => self.month,
            Part::Day => self.day,
            _ => self.year,
        }
    }
}

const OPEN_FIELDS: DateFields = DateFields {
    year: UpdateField::OpenYear,
    month: UpdateField::OpenMonth,
    day: UpdateField::OpenDay,
};
const EXPIRY_FIELDS: DateFields = DateFields {
    year: UpdateField::ExpiryYear,
    month: UpdateField::ExpiryMonth,
    day: UpdateField::ExpiryDay,
};
const REISSUE_FIELDS: DateFields = DateFields {
    year: UpdateField::ReissueYear,
    month: UpdateField::ReissueMonth,
    day: UpdateField::ReissueDay,
};
const DOB_FIELDS: DateFields = DateFields {
    year: UpdateField::DobYear,
    month: UpdateField::DobMonth,
    day: UpdateField::DobDay,
};

/// A date that passed its edit: the typed date of the edited year, month and day values
/// (COACTUPC-131), and the date the save writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DateChange {
    edited: NaiveDate,
    /// The received characters joined by `-` as a date: the edited date when all eight are
    /// digits; `None` when a part the edit accepted holds a space, a sign or a point, which no
    /// `DATE` column can hold (COACTUPC-111, X-COACTUPC-10).
    stored: Option<NaiveDate>,
}

/// The SSN a save writes (COACTUPC-064, COACTUPC-104).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SsnChange {
    /// The parts were omitted: the stored SSN stays.
    Unchanged,
    /// The re-entered nine digits.
    Replaced(Ssn),
}

/// The fields of a request whose 24 edits and cross edit passed: what `9600-WRITE-PROCESSING`
/// writes and what the screen then shows (COACTUPC-014, COACTUPC-103 to COACTUPC-111).
///
/// `Debug` shows no SSN digit: the re-entered parts and the edited SSN print redacted.
#[derive(Debug, Clone)]
pub struct ValidatedChanges {
    received: Received,
    credit_limit: Money<10>,
    cash_credit_limit: Money<10>,
    curr_bal: Money<10>,
    curr_cyc_credit: Money<10>,
    curr_cyc_debit: Money<10>,
    open_date: DateChange,
    expiry_date: DateChange,
    reissue_date: DateChange,
    ssn: SsnChange,
    dob: DateChange,
    fico: PicU<3>,
}

impl ValidatedChanges {
    /// The five amounts as the screen shows them: each validated value through
    /// `+ZZZ,ZZZ,ZZZ.99`, which a later F5 receives back (COACTUPC-078, COACTUPC-109).
    #[must_use]
    pub fn shown_amounts(&self) -> ShownAmounts {
        ShownAmounts {
            current_balance: currency(self.curr_bal),
            credit_limit: currency(self.credit_limit),
            cash_credit_limit: currency(self.cash_credit_limit),
            current_cycle_credit: currency(self.curr_cyc_credit),
            current_cycle_debit: currency(self.curr_cyc_debit),
        }
    }

    /// The edited open date (COACTUPC-016, COACTUPC-131).
    #[must_use]
    pub const fn open_date(&self) -> NaiveDate {
        self.open_date.edited
    }

    /// The edited expiry date (COACTUPC-018, COACTUPC-131).
    #[must_use]
    pub const fn expiry_date(&self) -> NaiveDate {
        self.expiry_date.edited
    }

    /// The edited reissue date (COACTUPC-020, COACTUPC-131).
    #[must_use]
    pub const fn reissue_date(&self) -> NaiveDate {
        self.reissue_date.edited
    }

    /// The edited date of birth (COACTUPC-025, COACTUPC-131).
    #[must_use]
    pub const fn dob(&self) -> NaiveDate {
        self.dob.edited
    }

    /// The five validated amounts in screen order: credit limit, cash credit limit, current
    /// balance, current cycle credit and current cycle debit (COACTUPC-009).
    #[must_use]
    pub const fn amounts(&self) -> [Money<10>; 5] {
        [
            self.credit_limit,
            self.cash_credit_limit,
            self.curr_bal,
            self.curr_cyc_credit,
            self.curr_cyc_debit,
        ]
    }
}

/// `1200-EDIT-MAP-INPUTS` after a change was found (COACTUPC-013 to COACTUPC-039,
/// COACTUPC-048 to COACTUPC-067), with CSUTLDTC's stand-in verdict on the four dates.
///
/// `HALTED(H5)`: [`validate`] decides which dates pass the LE step, and no test may present that
/// as the COBOL's (COACTUPC-129). Everything else is [`edit_with`]'s.
///
/// # Errors
///
/// The first edit that fails, as [`edit_with`] lists them.
pub fn edit(
    input: &AccountUpdateInput<'_>,
    current: &Fetched,
    clock: &dyn Clock,
) -> Result<ValidatedChanges, UpdateFailure> {
    edit_with(input, current, clock, validate)
}

/// [`edit`] with the date-validity verdict supplied by `verdict`, which receives the eight date
/// characters and the picture `YYYYMMDD` and returns CSUTLDTC's result (CSUTLDTC-019 to
/// CSUTLDTC-027).
///
/// The edits run in `1200-EDIT-MAP-INPUTS` order: Account Status, Open Date, Credit Limit, Expiry
/// Date, Cash Credit Limit, Reissue Date, Current Balance, Current Cycle Credit, Current Cycle
/// Debit, SSN, Date of Birth, FICO Score, First, Middle and Last Name, Address Line 1, State,
/// Zip, City, Country, Phone Number 1 and 2, EFT Account Id, Primary Card Holder, then the state
/// and ZIP cross edit. Each sets its message only while none is set, so the first that fails is
/// returned. Address line 2, the group ID, the customer ID and the government ID are not edited
/// (COACTUPC-039). Omitted SSN parts are edited as the parts of the stored SSN in `current`
/// (COACTUPC-064).
///
/// # Errors
///
/// Status 400 on the failing edit's field, in state `E`, with the amounts the screen shows
/// beside it (COACTUPC-013, COACTUPC-078). The messages are `TRIM(<name>)` followed by the
/// edit's literal: ` must be supplied.`, ` must be Y or N.`, ` is not valid`, ` can have
/// alphabets only.`, ` must be all numeric.`, ` must not be zero.`, `: should be between 300 and
/// 850`, `: is not a valid state code`, the date and phone literals of CSUTLDTC's edits, or
/// `Invalid zip code for state` (COACTUPC-015 to COACTUPC-038, COACTUPC-067).
pub fn edit_with(
    input: &AccountUpdateInput<'_>,
    current: &Fetched,
    clock: &dyn Clock,
    verdict: impl Fn(&str, DateMask) -> LeResult,
) -> Result<ValidatedChanges, UpdateFailure> {
    edit_received(Received::new(input), *current, clock, &verdict)
}

/// The edits over received fields; a failure leaves state `E` (COACTUPC-013).
fn edit_received(
    received: Received,
    current: Fetched,
    clock: &dyn Clock,
    verdict: &dyn Fn(&str, DateMask) -> LeResult,
) -> Result<ValidatedChanges, UpdateFailure> {
    let checked = edit_account_fields(&received, verdict)
        .and_then(|account| edit_customer_fields(&received, current, clock, verdict, account));
    match checked {
        Ok(fields) => Ok(fields.with(received)),
        Err(FieldError { message, field }) => Err(UpdateFailure::new(
            message,
            Some(field),
            FailureClass::Input,
            UpdateState::ChangesNotOk,
        )
        .with_shown(received.shown_amounts())),
    }
}

/// The values the account edits produced (edits 1 to 9).
struct AccountEdits {
    open_date: DateChange,
    credit_limit: Money<10>,
    expiry_date: DateChange,
    cash_credit_limit: Money<10>,
    reissue_date: DateChange,
    curr_bal: Money<10>,
    curr_cyc_credit: Money<10>,
    curr_cyc_debit: Money<10>,
}

/// The values every edit produced, before the received fields join them.
struct AllEdits {
    account: AccountEdits,
    ssn: SsnChange,
    dob: DateChange,
    fico: PicU<3>,
}

impl AllEdits {
    /// The validated changes of `received`.
    fn with(self, received: Received) -> ValidatedChanges {
        let Self {
            account,
            ssn,
            dob,
            fico,
        } = self;
        ValidatedChanges {
            received,
            credit_limit: account.credit_limit,
            cash_credit_limit: account.cash_credit_limit,
            curr_bal: account.curr_bal,
            curr_cyc_credit: account.curr_cyc_credit,
            curr_cyc_debit: account.curr_cyc_debit,
            open_date: account.open_date,
            expiry_date: account.expiry_date,
            reissue_date: account.reissue_date,
            ssn,
            dob,
            fico,
        }
    }
}

/// Edits 1 to 9 (COACTUPC-015 to COACTUPC-023).
fn edit_account_fields(
    new: &Received,
    verdict: &dyn Fn(&str, DateMask) -> LeResult,
) -> Result<AccountEdits, FieldError> {
    edit_yes_no(
        NAME_ACCOUNT_STATUS,
        &new.active_status,
        UpdateField::ActiveStatus,
    )?;
    let open_date = edit_date(NAME_OPEN_DATE, &new.open, OPEN_FIELDS, verdict)?;
    let credit_limit = edit_signed_9v2(
        NAME_CREDIT_LIMIT,
        &new.credit_limit,
        UpdateField::CreditLimit,
    )?;
    let expiry_date = edit_date(NAME_EXPIRY_DATE, &new.expiry, EXPIRY_FIELDS, verdict)?;
    let cash_credit_limit = edit_signed_9v2(
        NAME_CASH_CREDIT_LIMIT,
        &new.cash_credit_limit,
        UpdateField::CashCreditLimit,
    )?;
    let reissue_date = edit_date(NAME_REISSUE_DATE, &new.reissue, REISSUE_FIELDS, verdict)?;
    let curr_bal = edit_signed_9v2(
        NAME_CURRENT_BALANCE,
        &new.curr_bal,
        UpdateField::CurrentBalance,
    )?;
    let curr_cyc_credit = edit_signed_9v2(
        NAME_CYCLE_CREDIT,
        &new.curr_cyc_credit,
        UpdateField::CurrentCycleCredit,
    )?;
    let curr_cyc_debit = edit_signed_9v2(
        NAME_CYCLE_DEBIT,
        &new.curr_cyc_debit,
        UpdateField::CurrentCycleDebit,
    )?;
    Ok(AccountEdits {
        open_date,
        credit_limit,
        expiry_date,
        cash_credit_limit,
        reissue_date,
        curr_bal,
        curr_cyc_credit,
        curr_cyc_debit,
    })
}

/// Edits 10 to 24 and the cross edit (COACTUPC-024 to COACTUPC-038, COACTUPC-067).
fn edit_customer_fields(
    new: &Received,
    current: Fetched,
    clock: &dyn Clock,
    verdict: &dyn Fn(&str, DateMask) -> LeResult,
    account: AccountEdits,
) -> Result<AllEdits, FieldError> {
    let ssn = edit_ssn(new.ssn.as_ref(), current)?;
    let dob = edit_date(NAME_DATE_OF_BIRTH, &new.dob, DOB_FIELDS, verdict)?;
    edit_date_of_birth(NAME_DATE_OF_BIRTH, dob.edited, clock)
        .map_err(|failure| FieldError::from_edit(&failure, |part| DOB_FIELDS.of(part)))?;
    let fico = edit_fico(&new.fico)?;
    edit_alpha(
        NAME_FIRST_NAME,
        &new.first_name,
        UpdateField::FirstName,
        Presence::Required,
    )?;
    edit_alpha(
        NAME_MIDDLE_NAME,
        &new.middle_name,
        UpdateField::MiddleName,
        Presence::Optional,
    )?;
    edit_alpha(
        NAME_LAST_NAME,
        &new.last_name,
        UpdateField::LastName,
        Presence::Required,
    )?;
    edit_mandatory(
        NAME_ADDRESS_LINE_1,
        &new.address_line_1,
        UpdateField::AddressLine1,
    )?;
    edit_alpha(
        NAME_STATE,
        &new.state,
        UpdateField::State,
        Presence::Required,
    )?;
    edit_us_state(NAME_STATE, &new.state)
        .map_err(|failure| FieldError::from_edit(&failure, |_| UpdateField::State))?;
    edit_num_reqd(NAME_ZIP, &new.zip, W_ZIP_SHOWN)
        .map_err(|failure| FieldError::from_edit(&failure, |_| UpdateField::Zip))?;
    edit_alpha(NAME_CITY, &new.city, UpdateField::City, Presence::Required)?;
    edit_alpha(
        NAME_COUNTRY,
        &new.country,
        UpdateField::Country,
        Presence::Required,
    )?;
    edit_phone(NAME_PHONE_1, &new.phone_1, PHONE_1_FIELDS)?;
    edit_phone(NAME_PHONE_2, &new.phone_2, PHONE_2_FIELDS)?;
    edit_num_reqd(NAME_EFT_ACCOUNT_ID, &new.eft_account_id, W_EFT)
        .map_err(|failure| FieldError::from_edit(&failure, |_| UpdateField::EftAccountId))?;
    edit_yes_no(
        NAME_PRIMARY_CARD_HOLDER,
        &new.primary_card_holder,
        UpdateField::PrimaryCardHolder,
    )?;
    // The state and ZIP passed their edits to get here, the condition of the cross edit.
    edit_us_state_zip(&new.state, &new.zip)
        .map_err(|failure| FieldError::from_edit(&failure, |_| UpdateField::State))?;
    Ok(AllEdits {
        account,
        ssn,
        dob,
        fico,
    })
}

/// `1220-EDIT-YESNO` (COACTUPC-049): blank, a space or `0` must be supplied; anything but `Y`
/// and `N`, lower case included, must be Y or N.
fn edit_yes_no(name: &str, field: &str, target: UpdateField) -> Result<(), FieldError> {
    match field {
        "Y" | "N" => Ok(()),
        "\0" | " " | "0" => Err(FieldError::compose(name, LIT_SUPPLIED, target)),
        _ => Err(FieldError::compose(name, LIT_Y_OR_N, target)),
    }
}

/// `1250-EDIT-SIGNED-9V2` (COACTUPC-054): blank must be supplied; text `TEST-NUMVAL-C` rejects is
/// not valid; otherwise the value `1100-RECEIVE-MAP` stored (COACTUPC-009).
fn edit_signed_9v2(
    name: &str,
    amount: &Amount,
    target: UpdateField,
) -> Result<Money<10>, FieldError> {
    match amount.value {
        AmountValue::Value(value) => Ok(value),
        AmountValue::Blank => Err(FieldError::compose(name, LIT_SUPPLIED, target)),
        AmountValue::Rejected => Err(FieldError::compose(name, LIT_NOT_VALID, target)),
    }
}

/// `1215-EDIT-MANDATORY` (COACTUPC-048): blank must be supplied; any other text passes.
fn edit_mandatory(name: &str, field: &str, target: UpdateField) -> Result<(), FieldError> {
    if is_blank(field) {
        Err(FieldError::compose(name, LIT_SUPPLIED, target))
    } else {
        Ok(())
    }
}

/// Whether a letters edit requires its field: `1225-EDIT-ALPHA-REQD` or `1235-EDIT-ALPHA-OPT`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// `1225-EDIT-ALPHA-REQD`: a blank field must be supplied.
    Required,
    /// `1235-EDIT-ALPHA-OPT`: a blank field passes.
    Optional,
}

/// `1225-EDIT-ALPHA-REQD` and `1235-EDIT-ALPHA-OPT` (COACTUPC-050, COACTUPC-052): after the
/// blank test, every character must be `A` to `Z`, `a` to `z` or a space.
fn edit_alpha(
    name: &str,
    field: &str,
    target: UpdateField,
    presence: Presence,
) -> Result<(), FieldError> {
    if is_blank(field) {
        return match presence {
            Presence::Required => Err(FieldError::compose(name, LIT_SUPPLIED, target)),
            Presence::Optional => Ok(()),
        };
    }
    if field.chars().all(|c| c.is_ascii_alphabetic() || c == ' ') {
        Ok(())
    } else {
        Err(FieldError::compose(name, LIT_ALPHA_ONLY, target))
    }
}

/// `1245-EDIT-NUM-REQD` of the three FICO characters, then `1275-EDIT-FICO-SCORE`
/// (COACTUPC-026, COACTUPC-066).
fn edit_fico(field: &str) -> Result<PicU<3>, FieldError> {
    let target = UpdateField::FicoScore;
    edit_num_reqd(NAME_FICO_SCORE, field, W_FICO)
        .map_err(|failure| FieldError::from_edit(&failure, |_| target))?;
    match PicU::<3>::decode(field.as_bytes()) {
        Ok(score) if FICO_RANGE.contains(&score.get()) => Ok(score),
        // A three-digit text that `1245-EDIT-NUM-REQD` passed always decodes; only the range
        // test can fail here.
        _ => Err(FieldError::compose(NAME_FICO_SCORE, LIT_FICO_RANGE, target)),
    }
}

/// `EDIT-DATE-CCYYMMDD` of one date (COACTUPC-016, COACTUPC-018, COACTUPC-020, COACTUPC-025,
/// COACTUPC-117 to COACTUPC-131), with `verdict` for its LE step.
fn edit_date(
    name: &str,
    date: &DateIn,
    fields: DateFields,
    verdict: &dyn Fn(&str, DateMask) -> LeResult,
) -> Result<DateChange, FieldError> {
    let edited = edit_date_ccyymmdd_with(name, &date.year, &date.month, &date.day, verdict)
        .map_err(|failure| FieldError::from_edit(&failure, |part| fields.of(part)))?;
    let stored = date
        .text()
        .chars()
        .all(|c| c.is_ascii_digit())
        .then_some(edited);
    Ok(DateChange { edited, stored })
}

/// `1265-EDIT-US-SSN` (COACTUPC-024, COACTUPC-060 to COACTUPC-064): the re-entered parts, or the
/// stored SSN's parts when they were omitted.
fn edit_ssn(new: Option<&SsnText>, current: Fetched) -> Result<SsnChange, FieldError> {
    let field_of = |part| match part {
        Part::SsnMiddle2 => UpdateField::SsnPart2,
        Part::SsnLast4 => UpdateField::SsnPart3,
        _ => UpdateField::SsnPart1,
    };
    let edited = if let Some(SsnText([part1, part2, part3])) = new {
        edit_us_ssn(part1, part2, part3).map(|digits| SsnChange::Replaced(Ssn::new(digits)))
    } else {
        let stored = current.ssn.expose().get();
        let part1 = format!("{:03}", stored / 1_000_000);
        let part2 = format!("{:02}", stored / 10_000 % 100);
        let part3 = format!("{:04}", stored % 10_000);
        edit_us_ssn(&part1, &part2, &part3).map(|_| SsnChange::Unchanged)
    };
    edited.map_err(|failure| FieldError::from_edit(&failure, field_of))
}

/// The three map fields of one phone number.
#[derive(Clone, Copy)]
struct PhoneFields {
    area: UpdateField,
    prefix: UpdateField,
    line: UpdateField,
}

const PHONE_1_FIELDS: PhoneFields = PhoneFields {
    area: UpdateField::Phone1Area,
    prefix: UpdateField::Phone1Prefix,
    line: UpdateField::Phone1Line,
};
const PHONE_2_FIELDS: PhoneFields = PhoneFields {
    area: UpdateField::Phone2Area,
    prefix: UpdateField::Phone2Prefix,
    line: UpdateField::Phone2Line,
};

/// `1260-EDIT-US-PHONE-NUM` of one phone number (COACTUPC-035, COACTUPC-036, COACTUPC-055 to
/// COACTUPC-059).
fn edit_phone(name: &str, phone: &PhoneIn, fields: PhoneFields) -> Result<(), FieldError> {
    edit_us_phone(name, &phone.area, &phone.prefix, &phone.line).map_err(|failure| {
        FieldError::from_edit(&failure, |part| match part {
            Part::Prefix => fields.prefix,
            Part::LineNumber => fields.line,
            _ => fields.area,
        })
    })
}

/// A 200 answer of endpoint 6, the screen `3000-SEND-MAP` sends after it (COACTUPC-079,
/// COACTUPC-082).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// State `N`, info `Changes validated.Press F5 to save`, every field read-only and the
    /// `F5=Save` and `F12=Cancel` labels shown, nothing written (COACTUPC-014, COACTUPC-070). The
    /// fields show the values received, the amounts as given here (COACTUPC-078).
    ChangesValidated {
        /// The five amounts as shown.
        shown: ShownAmounts,
    },
    /// State `C`, info `Changes committed to database`, every field read-only and no `F5=Save` or
    /// `F12=Cancel` label: both records rewritten (COACTUPC-108).
    Committed {
        /// The five amounts as shown, the values saved.
        shown: ShownAmounts,
    },
    /// The empty screen of a new search: state not fetched, info `Enter or update id of account to
    /// update`. `0000-MAIN` answers so for any key in states `C`, `L` and `F` (COACTUPC-006); the
    /// replacement also answers so for an echoed state not fetched, which the screen never sends
    /// to endpoint 6 because it fetches through endpoint 5 then (COACTUPC-003).
    Restart,
}

impl UpdateOutcome {
    /// The state the screen is left in.
    #[must_use]
    pub const fn state(&self) -> UpdateState {
        match self {
            Self::ChangesValidated { .. } => UpdateState::ChangesOkNotConfirmed,
            Self::Committed { .. } => UpdateState::ChangesOkayedAndDone,
            Self::Restart => UpdateState::NotFetched,
        }
    }

    /// The info line of that state (COACTUPC-079).
    #[must_use]
    pub const fn info_message(&self) -> &'static str {
        self.state().info_message()
    }

    /// The five amounts as shown, when the screen shows the values received.
    #[must_use]
    pub const fn shown_amounts(&self) -> Option<&ShownAmounts> {
        match self {
            Self::ChangesValidated { shown } | Self::Committed { shown } => Some(shown),
            Self::Restart => None,
        }
    }
}

/// What [`decide`] asks of the service.
#[derive(Debug, Clone)]
pub enum Decision {
    /// Answer with this outcome; nothing is written.
    Respond(UpdateOutcome),
    /// Run the save of `9600-WRITE-PROCESSING` (COACTUPC-071, COACTUPC-101 to COACTUPC-112).
    Save(Box<PendingSave>),
}

/// `2000-DECIDE-ACTION` for endpoint 6 (COACTUPC-004, COACTUPC-012 to COACTUPC-014,
/// COACTUPC-043, COACTUPC-070, COACTUPC-071, COACTUPC-110), with the echoed state of `snapshot`
/// standing for `ACUP-CHANGE-ACTION`.
///
/// How each COBOL state maps to the request:
///
/// - `C`, `L`, `F`, and not fetched: [`UpdateOutcome::Restart`] (COACTUPC-006).
/// - `S` or `E`, with ENTER, or with F5, which `0000-MAIN` turns into ENTER outside state `N`
///   (COACTUPC-004): `1205-COMPARE-OLD-NEW` first; no change is `No change detected with respect
///   to values fetched.` in the echoed state (COACTUPC-043); a change runs the edits, whose first
///   failure leaves state `E` (COACTUPC-013) and whose success is
///   [`UpdateOutcome::ChangesValidated`] (COACTUPC-014).
/// - `N` with ENTER: the change check runs and no edit does; no change is the same 400 in state
///   `N`, a change is [`UpdateOutcome::ChangesValidated`] again (COACTUPC-012, COACTUPC-070).
/// - `N` with F5: the COBOL saves, receiving back the strings it showed. A REST client can send
///   anything, so the change check and the edits run again: with a change that passes,
///   [`Decision::Save`]; otherwise the answer an ENTER in state `S` gets (COACTUPC-110,
///   X-COACTUPC-1).
/// - An echoed code the program never sets: the change check runs; a change runs the edits as in
///   state `S`; no change reaches the `WHEN OTHER` of `2000-DECIDE-ACTION`, whose abend is a 500
///   with `UNEXPECTED DATA SCENARIO` ([`UpdateFailure::abend_data`]).
///
/// The four dates get CSUTLDTC's stand-in verdict, `HALTED(H5)` as in [`edit`];
/// [`decide_with`] takes the verdict as a parameter.
///
/// # Errors
///
/// The no-change 400, an edit's 400 as [`edit`] lists them, the abend's 500 above, and, when a
/// save would run with an account number that is not eleven digits, which the COBOL's read for
/// update cannot find, `Could not lock account record for update` (500, state `L`,
/// COACTUPC-101).
pub fn decide(
    aid: Aid,
    input: &AccountUpdateInput<'_>,
    snapshot: &Snapshot,
    current: &Fetched,
    clock: &dyn Clock,
) -> Result<Decision, UpdateFailure> {
    decide_with(aid, input, snapshot, current, clock, validate)
}

/// [`decide`] with the date-validity verdict supplied by `verdict`, as [`edit_with`] takes it
/// (COACTUPC-129).
///
/// # Errors
///
/// As [`decide`].
pub fn decide_with(
    aid: Aid,
    input: &AccountUpdateInput<'_>,
    snapshot: &Snapshot,
    current: &Fetched,
    clock: &dyn Clock,
    verdict: impl Fn(&str, DateMask) -> LeResult,
) -> Result<Decision, UpdateFailure> {
    let state = snapshot.state();
    if matches!(
        state,
        Some(
            UpdateState::NotFetched
                | UpdateState::ChangesOkayedAndDone
                | UpdateState::LockError
                | UpdateState::ChangesFailed
        )
    ) {
        return Ok(Decision::Respond(UpdateOutcome::Restart));
    }
    let received = Received::new(input);
    let change = compare_received(&received, snapshot, *current);
    match (aid, state, change) {
        (Aid::Pf5, Some(UpdateState::ChangesOkNotConfirmed), ChangeCheck::Changed) => {
            let changes = edit_received(received, *current, clock, &verdict)?;
            confirm_save(changes)
        }
        (Aid::Pf5, Some(UpdateState::ChangesOkNotConfirmed), ChangeCheck::Unchanged) => {
            Err(UpdateFailure::no_change(UpdateState::ShowDetails))
        }
        (_, Some(UpdateState::ChangesOkNotConfirmed), ChangeCheck::Changed) => {
            Ok(Decision::Respond(UpdateOutcome::ChangesValidated {
                shown: received.shown_amounts(),
            }))
        }
        (_, Some(echoed), ChangeCheck::Unchanged) => Err(UpdateFailure::no_change(echoed)),
        (_, None, ChangeCheck::Unchanged) => Err(UpdateFailure::unexpected_data()),
        (_, _, ChangeCheck::Changed) => {
            let changes = edit_received(received, *current, clock, &verdict)?;
            Ok(Decision::Respond(UpdateOutcome::ChangesValidated {
                shown: changes.shown_amounts(),
            }))
        }
    }
}

/// The save of validated changes, keyed by the account number received.
fn confirm_save(changes: ValidatedChanges) -> Result<Decision, UpdateFailure> {
    match PicU::<11>::decode(changes.received.account_id.as_bytes()) {
        Ok(account_key) => Ok(Decision::Save(Box::new(PendingSave {
            account_key,
            changes,
        }))),
        Err(_) => Err(PendingSave::lock_account_failure(&changes)),
    }
}

/// Validated changes F5 confirmed: what the service saves (COACTUPC-101 to COACTUPC-112).
#[derive(Debug, Clone)]
pub struct PendingSave {
    account_key: PicU<11>,
    changes: ValidatedChanges,
}

impl PendingSave {
    /// The key of the account's read for update: the account number received, the path's
    /// (COACTUPC-112).
    #[must_use]
    pub const fn account_key(&self) -> PicU<11> {
        self.account_key
    }

    /// The validated changes.
    #[must_use]
    pub const fn changes(&self) -> &ValidatedChanges {
        &self.changes
    }

    /// The failure of the account's read for update: 500 `Could not lock account record for
    /// update`, state `L`, nothing written (COACTUPC-101).
    #[must_use]
    pub fn account_lock_failed(&self) -> UpdateFailure {
        Self::lock_account_failure(&self.changes)
    }

    /// [`PendingSave::account_lock_failed`] for `changes`.
    fn lock_account_failure(changes: &ValidatedChanges) -> UpdateFailure {
        UpdateFailure::new(
            LOCK_ACCOUNT_FAILED,
            None,
            FailureClass::Store,
            UpdateState::LockError,
        )
        .with_shown(changes.shown_amounts())
    }

    /// The failure of the customer's read for update: 500 `Could not lock customer record for
    /// update`, nothing written, and state `C`, because `2000-DECIDE-ACTION` has no branch for
    /// it and its `WHEN OTHER` sets the state of a successful save (COACTUPC-102, X-COACTUPC-6).
    #[must_use]
    pub fn customer_lock_failed(&self) -> UpdateFailure {
        UpdateFailure::new(
            LOCK_CUSTOMER_FAILED,
            None,
            FailureClass::Store,
            UpdateState::ChangesOkayedAndDone,
        )
        .with_shown(self.changes.shown_amounts())
    }

    /// The failure of a rewrite: 500 `Update of record failed`, state `F` (COACTUPC-106,
    /// COACTUPC-107). When the customer rewrite fails after the account rewrite succeeded, the
    /// COBOL's `SYNCPOINT ROLLBACK` backs out nothing, because both files are `RECOVERY(NONE)`:
    /// the account keeps its new values (X-COACTUPC-7).
    #[must_use]
    pub fn rewrite_failed(&self) -> UpdateFailure {
        UpdateFailure::new(
            UPDATE_FAILED,
            None,
            FailureClass::Store,
            UpdateState::ChangesFailed,
        )
        .with_shown(self.changes.shown_amounts())
    }

    /// Both rewrites succeeded: state `C` (COACTUPC-108).
    #[must_use]
    pub fn committed(&self) -> UpdateOutcome {
        UpdateOutcome::Committed {
            shown: self.changes.shown_amounts(),
        }
    }

    /// The two records `9600-WRITE-PROCESSING` rewrites over the ones it read for update
    /// (COACTUPC-103 to COACTUPC-105, COACTUPC-109, COACTUPC-111, COACTUPC-112).
    ///
    /// The account takes the status, the five amounts and the three dates received, and the
    /// group ID received lands in `ACCT-ADDR-ZIP`, where the update record's layout puts it; the
    /// stored `ACCT-GROUP-ID` and `FILLER` are kept, where the COBOL writes uninitialised storage
    /// (X-COACTUPC-4, halt candidate `X-COACTUPC-H2`). The customer takes every field received:
    /// the ZIP as its five characters and five spaces, each phone number as `(`, area code, `)`,
    /// prefix, `-`, line number and two spaces, the SSN re-entered or, when omitted, the stored
    /// one, the date of birth and the FICO score; its `FILLER` is kept. A field sent blank is
    /// written as spaces where the COBOL writes low-values, which no column can hold
    /// (X-COACTUPC-5). A received customer ID that is not the key read for update makes the
    /// customer rewrite fail, as CICS refuses a key change: [`CustomerRewrite::KeyChanged`]
    /// (COACTUPC-112).
    ///
    /// # Errors
    ///
    /// [`PendingSave::rewrite_failed`] before any write when a value cannot be stored: a date
    /// whose received parts are not all digits, such as a day `1.` the edit accepted
    /// (COACTUPC-111, X-COACTUPC-10), a field holding a character outside printable ASCII, or an
    /// account key that is not the key read for update. None of these is a parity result.
    pub fn save_effects(
        &self,
        locked_account: &AccountRecord,
        locked_customer: &CustomerRecord,
    ) -> Result<SaveEffects, UpdateFailure> {
        let account = self
            .account_record(locked_account)
            .ok_or_else(|| self.rewrite_failed())?;
        let customer = match self.customer_record(locked_customer) {
            CustomerBuild::Record(record) => CustomerRewrite::Record(record),
            CustomerBuild::KeyChanged => CustomerRewrite::KeyChanged,
            CustomerBuild::Unwritable => return Err(self.rewrite_failed()),
        };
        Ok(SaveEffects { account, customer })
    }

    /// The account `9600-WRITE-PROCESSING` rewrites (COACTUPC-103, COACTUPC-109, COACTUPC-111);
    /// `None` when a value cannot be stored.
    fn account_record(&self, locked: &AccountRecord) -> Option<AccountRecord> {
        let changes = &self.changes;
        let received = &changes.received;
        if locked.id != self.account_key {
            return None;
        }
        Some(AccountRecord {
            id: self.account_key,
            active_status: written(&received.active_status)?,
            curr_bal: changes.curr_bal,
            credit_limit: changes.credit_limit,
            cash_credit_limit: changes.cash_credit_limit,
            open_date: stored_date(changes.open_date)?,
            expiraion_date: stored_date(changes.expiry_date)?,
            reissue_date: stored_date(changes.reissue_date)?,
            curr_cyc_credit: changes.curr_cyc_credit,
            curr_cyc_debit: changes.curr_cyc_debit,
            // The update record has no ZIP: the group ID received lands in its place
            // (COACTUPC-103, X-COACTUPC-4).
            addr_zip: written(&received.group_id)?,
            // Halt candidate X-COACTUPC-H2: the COBOL writes storage no statement sets.
            group_id: locked.group_id.clone(),
            filler: locked.filler.clone(),
        })
    }

    /// The customer `9600-WRITE-PROCESSING` rewrites (COACTUPC-104, COACTUPC-105, COACTUPC-111,
    /// COACTUPC-112).
    fn customer_record(&self, locked: &CustomerRecord) -> CustomerBuild {
        let changes = &self.changes;
        let received = &changes.received;
        let ssn = match changes.ssn {
            SsnChange::Unchanged => locked.ssn,
            SsnChange::Replaced(ssn) => ssn,
        };
        let built = || {
            Some(CustomerRecord {
                id: locked.id,
                first_name: written(&received.first_name)?,
                middle_name: written(&received.middle_name)?,
                last_name: written(&received.last_name)?,
                addr_line_1: written(&received.address_line_1)?,
                addr_line_2: written(&received.address_line_2)?,
                addr_line_3: written(&received.city)?,
                addr_state_cd: written(&received.state)?,
                addr_country_cd: written(&received.country)?,
                addr_zip: written(&received.zip)?,
                phone_num_1: received.phone_1.written()?,
                phone_num_2: received.phone_2.written()?,
                ssn,
                govt_issued_id: written(&received.government_id)?,
                dob_yyyy_mm_dd: stored_date(changes.dob)?,
                eft_account_id: written(&received.eft_account_id)?,
                pri_card_holder_ind: written(&received.primary_card_holder)?,
                fico_credit_score: changes.fico,
                // Halt candidate X-COACTUPC-H2: the COBOL writes storage no statement sets.
                filler: locked.filler.clone(),
            })
        };
        match built() {
            None => CustomerBuild::Unwritable,
            Some(record) => match PicU::<9>::decode(received.customer_id.as_bytes()) {
                Ok(id) if id == locked.id => CustomerBuild::Record(Box::new(record)),
                _ => CustomerBuild::KeyChanged,
            },
        }
    }
}

/// A date a save writes: the typed date when the received parts were its digits.
fn stored_date(date: DateChange) -> Option<PicDate> {
    date.stored.and_then(|value| PicDate::new(Some(value)).ok())
}

/// What [`PendingSave::save_effects`] made of the customer.
enum CustomerBuild {
    /// The record to rewrite.
    Record(Box<CustomerRecord>),
    /// The customer ID received is not the key read for update.
    KeyChanged,
    /// A value cannot be stored.
    Unwritable,
}

/// The customer rewrite of a save (COACTUPC-104, COACTUPC-107, COACTUPC-112).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomerRewrite {
    /// Rewrite the customer with this record, after the account rewrite succeeded.
    Record(Box<CustomerRecord>),
    /// The customer ID received is not the key read for update, so the rewrite fails as CICS
    /// fails a key change: the service rewrites the account, writes no customer and answers
    /// [`PendingSave::rewrite_failed`], the account update standing (COACTUPC-107,
    /// COACTUPC-112, X-COACTUPC-7).
    KeyChanged,
}

/// The writes of a save, in COBOL order: the account, then the customer (COACTUPC-103,
/// COACTUPC-104).
///
/// The service rewrites [`SaveEffects::account`] first; when that fails it answers
/// [`PendingSave::rewrite_failed`] and writes nothing more (COACTUPC-106). It then performs
/// [`SaveEffects::customer`]; a failure, or [`CustomerRewrite::KeyChanged`], is
/// [`PendingSave::rewrite_failed`] with the account rewrite kept (COACTUPC-107). Both
/// succeeding is [`PendingSave::committed`] (COACTUPC-108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEffects {
    account: AccountRecord,
    customer: CustomerRewrite,
}

impl SaveEffects {
    /// The account record to rewrite.
    #[must_use]
    pub const fn account(&self) -> &AccountRecord {
        &self.account
    }

    /// The customer rewrite.
    #[must_use]
    pub const fn customer(&self) -> &CustomerRewrite {
        &self.customer
    }

    /// The two writes, by value.
    #[must_use]
    pub fn into_parts(self) -> (AccountRecord, CustomerRewrite) {
        (self.account, self.customer)
    }
}

/// `9700-CHECK-CHANGE-IN-REC` (COACTUPC-113 to COACTUPC-116): the records the save read for
/// update against the values the echoed `snapshot` holds from the fetch.
///
/// The account terms first: the status exactly, letter case counting; the five amounts by
/// value; the open, expiry and reissue dates; the group ID after `LOWER-CASE`. The account ID and
/// `ACCT-ADDR-ZIP` are not compared. Then the customer terms: after `UPPER-CASE` the three names,
/// the three address lines, the state, the country and the government ID; exactly the ZIP, both
/// phone numbers, the date of birth, the EFT account ID, the primary-holder indicator and the
/// FICO score; the SSN by its last four digits only, the snapshot's whole SSN content (E32,
/// COACTUPC-115). The customer ID is not compared. A snapshot value that is unknown equals
/// nothing.
///
/// # Errors
///
/// Any difference: status 409, `Record changed by some one else. Please review`, nothing
/// written, state `S` (COACTUPC-113, COACTUPC-116). The response shows only the message; the
/// stale snapshot is kept, as `9700` does not refresh `ACUP-OLD-DETAILS`.
pub fn check_change_in_rec(
    snapshot: &Snapshot,
    current: (&AccountRecord, &CustomerRecord),
) -> Result<(), UpdateFailure> {
    let (account, customer) = current;
    if account_unchanged(snapshot, account) && customer_unchanged(snapshot, customer) {
        Ok(())
    } else {
        Err(UpdateFailure::changed())
    }
}

/// The account terms of `9700-CHECK-CHANGE-IN-REC` (COACTUPC-113).
fn account_unchanged(old: &Snapshot, account: &AccountRecord) -> bool {
    account.active_status == old.active_status
        && old.curr_bal == Some(account.curr_bal)
        && old.credit_limit == Some(account.credit_limit)
        && old.cash_credit_limit == Some(account.cash_credit_limit)
        && old.curr_cyc_credit == Some(account.curr_cyc_credit)
        && old.curr_cyc_debit == Some(account.curr_cyc_debit)
        && old.open_date == account.open_date.value()
        && old.expiraion_date == account.expiraion_date.value()
        && old.reissue_date == account.reissue_date.value()
        && case_free(account.group_id.as_bytes(), old.group_id.as_bytes())
}

/// The customer terms of `9700-CHECK-CHANGE-IN-REC` (COACTUPC-114, COACTUPC-115).
fn customer_unchanged(old: &Snapshot, customer: &CustomerRecord) -> bool {
    case_free(customer.first_name.as_bytes(), old.first_name.as_bytes())
        && case_free(customer.middle_name.as_bytes(), old.middle_name.as_bytes())
        && case_free(customer.last_name.as_bytes(), old.last_name.as_bytes())
        && case_free(customer.addr_line_1.as_bytes(), old.addr_line_1.as_bytes())
        && case_free(customer.addr_line_2.as_bytes(), old.addr_line_2.as_bytes())
        && case_free(customer.addr_line_3.as_bytes(), old.addr_line_3.as_bytes())
        && case_free(
            customer.addr_state_cd.as_bytes(),
            old.addr_state_cd.as_bytes(),
        )
        && case_free(
            customer.addr_country_cd.as_bytes(),
            old.addr_country_cd.as_bytes(),
        )
        && customer.addr_zip == old.addr_zip
        && customer.phone_num_1 == old.phone_num_1
        && customer.phone_num_2 == old.phone_num_2
        && old.ssn_last4 == Some(customer.ssn.last4())
        && case_free(
            customer.govt_issued_id.as_bytes(),
            old.govt_issued_id.as_bytes(),
        )
        && old.dob == customer.dob_yyyy_mm_dd.value()
        && customer.eft_account_id == old.eft_account_id
        && customer.pri_card_holder_ind == old.pri_card_holder_ind
        && old.fico_credit_score == Some(customer.fico_credit_score)
}

/// `FUNCTION UPPER-CASE (a) EQUAL FUNCTION UPPER-CASE (b)`, and the same under `LOWER-CASE`, for
/// two fields of one width: equal but for the case of `A` to `Z`.
fn case_free(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use copybook::{PicDate, PicU, PicX};

    use super::super::csutldtc::{DateMask, LeFeedback, LeResult, validate};
    use super::{
        AccountUpdateInput, AccountUpdateView, Aid, ChangeCheck, CustomerRewrite, DateParts,
        Decision, FailureClass, Fetched, Lookup, PendingSave, PhoneParts, SaveEffects, Snapshot,
        SnapshotFields, SsnParts, UpdateFailure, UpdateField, UpdateOutcome, UpdateState,
        ValidatedChanges, account_key, check_change_in_rec, compare_old_new, currency, decide,
        decide_with, edit, edit_with, fetch, numval_c,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money};
    use crate::records::account::AccountRecord;
    use crate::records::card_xref::CardXrefRecord;
    use crate::records::customer::CustomerRecord;
    use crate::sensitive::{CardNumber, Ssn};

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field and HTTP status.
    type Shown = (String, Option<UpdateField>, u16);

    /// One change a test makes to a form.
    type Touch = fn(&mut Form);

    // The audited literals, written out apart from the module's own constants.
    const NO_INPUT: &str = "No input received";
    const NOT_VALID_KEY: &str = "Account Number if supplied must be a 11 digit Non-Zero Number";
    const NO_CHANGE: &str = "No change detected with respect to values fetched.";
    const VALIDATED: &str = "Changes validated.Press F5 to save";
    const COMMITTED: &str = "Changes committed to database";
    const UNSUCCESSFUL: &str = "Changes unsuccessful. Please try again";
    const PRESENTED: &str = "Update account details presented above.";
    const SEARCH: &str = "Enter or update id of account to update";
    const LOCK_ACCOUNT: &str = "Could not lock account record for update";
    const LOCK_CUSTOMER: &str = "Could not lock customer record for update";
    const CHANGED: &str = "Record changed by some one else. Please review";
    const UPDATE_FAILED: &str = "Update of record failed";

    /// The stored SSN of the test customer, in no response.
    const SSN: u64 = 123_456_789;

    fn money(cents: i64) -> Result<Money<10>, String> {
        Money::new(Decimal::new(cents, 2)).map_err(|e| format!("{e:?}"))
    }

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, String> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| format!("{year}-{month}-{day}"))
    }

    fn pic_date(year: i32, month: u32, day: u32) -> Result<PicDate, String> {
        PicDate::new(Some(date(year, month, day)?)).map_err(|e| format!("{e:?}"))
    }

    fn text<const N: usize>(value: &str) -> Result<PicX<N>, String> {
        PicX::from_text(value).map_err(|e| format!("{value:?}: {e:?}"))
    }

    fn digits<const N: u32>(value: u64) -> Result<PicU<N>, String> {
        PicU::new(value).map_err(|e| format!("{value}: {e:?}"))
    }

    /// The clock the date-of-birth check reads: 2022-07-18, 13:45:07.
    fn clock() -> Result<FixedClock, String> {
        let at = date(2022, 7, 18)?
            .and_hms_opt(13, 45, 7)
            .ok_or("no such time")?;
        Ok(FixedClock::new(at))
    }

    /// A verdict that accepts every date, substituted for the H5 call: the rules around the LE
    /// step are proved for every accepting verdict, and no date validity is asserted.
    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    /// A verdict that rejects every date, substituted for the H5 call.
    fn reject_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::BadDateValue)
    }

    /// Account 1 as stored: a group ID that is not blank, so an untouched form is no change.
    fn account() -> Result<AccountRecord, String> {
        Ok(AccountRecord {
            id: digits(1)?,
            active_status: text("Y")?,
            curr_bal: money(19_400)?,
            credit_limit: money(202_000)?,
            cash_credit_limit: money(102_000)?,
            open_date: pic_date(2014, 11, 20)?,
            expiraion_date: pic_date(2025, 5, 20)?,
            reissue_date: pic_date(2025, 5, 20)?,
            curr_cyc_credit: money(0)?,
            curr_cyc_debit: money(-5_025)?,
            addr_zip: text("A000000000")?,
            group_id: text("GOLD")?,
            filler: text("STORED ACCOUNT FILLER")?,
        })
    }

    /// Customer 1 as stored, with a state and ZIP pair and area codes the tables accept.
    fn customer() -> Result<CustomerRecord, String> {
        Ok(CustomerRecord {
            id: digits(1)?,
            first_name: text("Immanuel")?,
            middle_name: text("Madeline")?,
            last_name: text("Kessler")?,
            addr_line_1: text("618 Deshaun Route")?,
            addr_line_2: text("Apt. 802")?,
            addr_line_3: text("Altenwerthshire")?,
            addr_state_cd: text("CA")?,
            addr_country_cd: text("USA")?,
            addr_zip: text("90210")?,
            phone_num_1: text("(201)555-1234")?,
            phone_num_2: text("(202)555-9876")?,
            ssn: Ssn::new(digits(SSN)?),
            govt_issued_id: text("000000000004936843")?,
            dob_yyyy_mm_dd: pic_date(1961, 6, 8)?,
            eft_account_id: text("0053581756")?,
            pri_card_holder_ind: text("Y")?,
            fico_credit_score: digits(750)?,
            filler: text("STORED CUSTOMER FILLER")?,
        })
    }

    fn xref() -> Result<CardXrefRecord, String> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(text("9999000011112222")?),
            cust_id: digits(1)?,
            acct_id: digits(1)?,
            filler: PicX::spaces(),
        })
    }

    /// The fields of an endpoint 6 request as owned text, which a test edits before building the
    /// borrowed [`AccountUpdateInput`].
    #[derive(Clone)]
    struct Form {
        account_id: String,
        active_status: String,
        open: [String; 3],
        credit_limit: String,
        expiry: [String; 3],
        cash_credit_limit: String,
        reissue: [String; 3],
        current_balance: String,
        current_cycle_credit: String,
        group_id: String,
        current_cycle_debit: String,
        customer_id: String,
        ssn: Option<[String; 3]>,
        dob: [String; 3],
        fico: String,
        names: [String; 3],
        address_line_1: String,
        state: String,
        address_line_2: String,
        zip: String,
        city: String,
        country: String,
        phone_1: [String; 3],
        government_id: String,
        phone_2: [String; 3],
        eft: String,
        primary: String,
    }

    fn parts(value: [&str; 3]) -> [String; 3] {
        value.map(str::to_owned)
    }

    fn date_parts(value: &[String; 3]) -> DateParts<'_> {
        let [year, month, day] = value;
        DateParts { year, month, day }
    }

    fn phone_parts(value: &[String; 3]) -> PhoneParts<'_> {
        let [area, prefix, line] = value;
        PhoneParts { area, prefix, line }
    }

    impl Form {
        /// The form as the screen fills it from endpoint 5's view of [`account`] and
        /// [`customer`], the SSN not re-entered.
        fn shown() -> Result<Self, String> {
            let account = account()?;
            Ok(Self {
                account_id: "00000000001".to_owned(),
                active_status: "Y".to_owned(),
                open: parts(["2014", "11", "20"]),
                credit_limit: currency(account.credit_limit),
                expiry: parts(["2025", "05", "20"]),
                cash_credit_limit: currency(account.cash_credit_limit),
                reissue: parts(["2025", "05", "20"]),
                current_balance: currency(account.curr_bal),
                current_cycle_credit: currency(account.curr_cyc_credit),
                group_id: "GOLD".to_owned(),
                current_cycle_debit: currency(account.curr_cyc_debit),
                customer_id: "000000001".to_owned(),
                ssn: None,
                dob: parts(["1961", "06", "08"]),
                fico: "750".to_owned(),
                names: parts(["Immanuel", "Madeline", "Kessler"]),
                address_line_1: "618 Deshaun Route".to_owned(),
                state: "CA".to_owned(),
                address_line_2: "Apt. 802".to_owned(),
                zip: "90210".to_owned(),
                city: "Altenwerthshire".to_owned(),
                country: "USA".to_owned(),
                phone_1: parts(["201", "555", "1234"]),
                government_id: "000000000004936843".to_owned(),
                phone_2: parts(["202", "555", "9876"]),
                eft: "0053581756".to_owned(),
                primary: "Y".to_owned(),
            })
        }

        /// The form with one change an edit accepts: a new last name.
        fn changed() -> Result<Self, String> {
            let mut form = Self::shown()?;
            form.names[2] = "Kessling".to_owned();
            Ok(form)
        }

        fn input(&self) -> AccountUpdateInput<'_> {
            let [first_name, middle_name, last_name] = &self.names;
            AccountUpdateInput {
                account_id: &self.account_id,
                active_status: &self.active_status,
                open_date: date_parts(&self.open),
                credit_limit: &self.credit_limit,
                expiry_date: date_parts(&self.expiry),
                cash_credit_limit: &self.cash_credit_limit,
                reissue_date: date_parts(&self.reissue),
                current_balance: &self.current_balance,
                current_cycle_credit: &self.current_cycle_credit,
                group_id: &self.group_id,
                current_cycle_debit: &self.current_cycle_debit,
                customer_id: &self.customer_id,
                ssn: self.ssn.as_ref().map(|[part1, part2, part3]| SsnParts {
                    part1,
                    part2,
                    part3,
                }),
                dob: date_parts(&self.dob),
                fico_score: &self.fico,
                first_name,
                middle_name,
                last_name,
                address_line_1: &self.address_line_1,
                state: &self.state,
                address_line_2: &self.address_line_2,
                zip: &self.zip,
                city: &self.city,
                country: &self.country,
                phone_1: phone_parts(&self.phone_1),
                government_id: &self.government_id,
                phone_2: phone_parts(&self.phone_2),
                eft_account_id: &self.eft,
                primary_card_holder: &self.primary,
            }
        }
    }

    /// The snapshot of the stored records, as the client echoes it in `state`.
    fn snapshot(state: UpdateState) -> Result<Snapshot, String> {
        Ok(Snapshot::from_records(&account()?, &customer()?).with_state(state))
    }

    fn fetched() -> Result<Fetched, String> {
        Ok(Fetched::new(&customer()?))
    }

    fn shown(failure: &UpdateFailure) -> Shown {
        (
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
        )
    }

    fn input_error(message: &str, field: UpdateField) -> Shown {
        (message.to_owned(), Some(field), 400)
    }

    /// The edit of `form` with the accepting verdict.
    fn edited(form: &Form) -> Result<Result<ValidatedChanges, UpdateFailure>, String> {
        Ok(edit_with(&form.input(), &fetched()?, &clock()?, accept_all))
    }

    /// The first failure the edits of `form` return.
    fn edit_failure(form: &Form) -> Result<Shown, String> {
        match edited(form)? {
            Ok(_) => Err("the edits passed".to_owned()),
            Err(failure) => {
                if failure.state() != UpdateState::ChangesNotOk {
                    return Err(format!("state {:?}", failure.state()));
                }
                Ok(shown(&failure))
            }
        }
    }

    /// `decide_with` for `form` with key `aid` in echoed state `state`, accepting every date.
    fn decided(
        aid: Aid,
        form: &Form,
        state: UpdateState,
    ) -> Result<Result<Decision, UpdateFailure>, String> {
        Ok(decide_with(
            aid,
            &form.input(),
            &snapshot(state)?,
            &fetched()?,
            &clock()?,
            accept_all,
        ))
    }

    /// The save F5 in state `N` asks for with `form`.
    fn pending(form: &Form) -> Result<PendingSave, String> {
        match decided(Aid::Pf5, form, UpdateState::ChangesOkNotConfirmed)? {
            Ok(Decision::Save(save)) => Ok(*save),
            Ok(Decision::Respond(outcome)) => Err(format!("responded {outcome:?}")),
            Err(failure) => Err(format!("failed {:?}", shown(&failure))),
        }
    }

    /// The writes of the save of `form` over the stored records.
    fn effects(form: &Form) -> Result<Result<SaveEffects, UpdateFailure>, String> {
        Ok(pending(form)?.save_effects(&account()?, &customer()?))
    }

    #[test]
    fn account_key_edits_the_path_number() -> TestResult {
        // audit: COACTUPC-045
        for blank in ["", "           ", "*          ", "\0\0\0\0\0\0\0\0\0\0\0"] {
            let failure = account_key(blank).err().ok_or("blank passed")?;
            assert_eq!(
                shown(&failure),
                input_error(NO_INPUT, UpdateField::AccountId)
            );
            assert_eq!(failure.state(), UpdateState::NotFetched);
            assert_eq!(failure.state().info_message(), SEARCH);
        }
        // audit: COACTUPC-046
        for bad in [
            "123",
            "0000000000A",
            "00000000000",
            "-0000000001",
            "1.000000000",
        ] {
            let failure = account_key(bad).err().ok_or("bad key passed")?;
            assert_eq!(
                shown(&failure),
                input_error(NOT_VALID_KEY, UpdateField::AccountId)
            );
        }
        // audit: COACTUPC-047
        assert_eq!(
            account_key("00000000001").map_err(|f| f.message().to_owned())?,
            digits(1)?
        );
        // audit: COACTUPC-011
        let failure = fetch("", Lookup::NotFound, Lookup::NotFound, Lookup::NotFound)
            .err()
            .ok_or("blank fetched")?;
        assert_eq!(failure.message(), NO_INPUT);
        Ok(())
    }

    #[test]
    fn fetch_shows_the_account_and_customer() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        // audit: COACTUPC-068, COACTUPC-074, COACTUPC-075, COACTUPC-077
        let view = fetch(
            "00000000001",
            Lookup::Found(&xref),
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )
        .map_err(|f| f.message().to_owned())?;
        assert_eq!(view.info_message(), PRESENTED);
        assert_eq!(AccountUpdateView::STATE, UpdateState::ShowDetails);
        assert_eq!(view.amounts.credit_limit, "+      2,020.00");
        assert_eq!(view.amounts.current_cycle_debit, "-         50.25");
        assert_eq!(view.open_date, Some(date(2014, 11, 20)?));
        assert_eq!(view.dob, Some(date(1961, 6, 8)?));
        assert_eq!(view.zip.as_bytes(), b"90210");
        assert_eq!(view.phone_1.area.as_bytes(), b"201");
        assert_eq!(view.phone_1.prefix.as_bytes(), b"555");
        assert_eq!(view.phone_1.line.as_bytes(), b"1234");
        assert_eq!(view.group_id.as_bytes(), b"GOLD      ");
        assert_eq!(view.snapshot.state(), Some(UpdateState::ShowDetails));
        // audit: COACTUPC-076, COACTUPC-090
        let debug = format!("{view:?}");
        assert!(!debug.contains("123456789") && !debug.contains("123-45"));
        assert!(debug.contains("***-**-6789"));
        assert!(!debug.contains("9999000011112222"));
        Ok(())
    }

    #[test]
    fn fetch_answers_the_cross_reference_failures() -> TestResult {
        let (account, customer) = (account()?, customer()?);
        // audit: COACTUPC-085
        let missing = fetch(
            "00000000001",
            Lookup::NotFound,
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )
        .err()
        .ok_or("fetched without a cross-reference")?;
        // HALTED(H4): only the positions outside the CICS codes are asserted.
        let message = missing.message();
        assert!(message.starts_with("Account:00000000001 not found in Cross ref file.  Resp:"));
        assert_eq!(message.get(65..71), Some(" Reas:"));
        assert_eq!(missing.field(), Some(UpdateField::AccountId));
        assert_eq!(missing.class(), FailureClass::NotFound);
        assert_eq!(missing.state(), UpdateState::NotFetched);
        // audit: COACTUPC-087, COACTUPC-089
        let failed = fetch(
            "00000000001",
            Lookup::Failed,
            Lookup::Found(&account),
            Lookup::Found(&customer),
        )
        .err()
        .ok_or("fetched after a failed read")?;
        let message = failed.message();
        assert!(message.starts_with("File Error: READ     on CXACAIX   returned RESP "));
        // The code positions are spaces, which the trailing trim removes with the space ending
        // `,RESP2 `.
        assert_eq!(message.get(58..), Some(",RESP2"));
        assert_eq!(shown(&failed).2, 500);
        assert_eq!(failed.field(), Some(UpdateField::AccountId));
        Ok(())
    }

    #[test]
    fn fetch_answers_the_account_and_customer_failures() -> TestResult {
        let (account, customer, xref) = (account()?, customer()?, xref()?);
        let key = "00000000001";
        let found = Lookup::Found(&xref);
        // audit: COACTUPC-091
        let failure = fetch(key, found, Lookup::NotFound, Lookup::NotFound)
            .err()
            .ok_or("fetched without an account")?;
        assert!(
            failure
                .message()
                .starts_with("Account:00000000001 not found in Acct Master file.Resp:")
        );
        assert_eq!(status_of(&failure), (404, Some(UpdateField::AccountId)));
        // audit: COACTUPC-095
        let failure = fetch(key, found, Lookup::Found(&account), Lookup::NotFound)
            .err()
            .ok_or("fetched without a customer")?;
        assert!(
            failure
                .message()
                .starts_with("CustId:000000001 not found in customer master.Resp: ")
        );
        assert_eq!(failure.message().get(62..68), Some(" REAS:"));
        assert_eq!(status_of(&failure), (404, None));
        // audit: COACTUPC-097
        let failure = fetch(key, found, Lookup::Found(&account), Lookup::Failed)
            .err()
            .ok_or("fetched after a failed customer read")?;
        assert!(
            failure
                .message()
                .starts_with("File Error: READ     on CUSTDAT   ")
        );
        assert_eq!(status_of(&failure), (500, None));
        let failure = fetch(key, found, Lookup::NotFound, Lookup::Failed)
            .err()
            .ok_or("fetched after two failed reads")?;
        assert!(failure.message().contains(" on CUSTDAT "));
        assert_eq!(status_of(&failure), (500, Some(UpdateField::AccountId)));
        // audit: COACTUPC-093, COACTUPC-099
        let failure = fetch(key, found, Lookup::Failed, Lookup::Found(&customer))
            .err()
            .ok_or("fetched after a failed account read")?;
        assert!(failure.message().contains(" on ACCTDAT "));
        assert_eq!(status_of(&failure), (500, Some(UpdateField::AccountId)));
        assert_eq!(failure.state(), UpdateState::ShowDetails);
        let details = failure.details().ok_or("no customer block")?;
        assert_eq!(details.first_name, customer.first_name);
        Ok(())
    }

    /// The HTTP status and the field of a failure.
    fn status_of(failure: &UpdateFailure) -> (u16, Option<UpdateField>) {
        (failure.class().http_status(), failure.field())
    }

    #[test]
    fn snapshot_round_trips_and_holds_the_last_four_digits() -> TestResult {
        // audit: COACTUPC-100
        let snapshot = snapshot(UpdateState::ShowDetails)?;
        let fields = snapshot.fields();
        assert_eq!(fields.state, "S");
        assert_eq!(fields.account_id, "00000000001");
        assert_eq!(fields.current_cycle_debit, "-50.25");
        assert_eq!(fields.ssn_last4, "6789");
        assert_eq!(fields.zip, "90210     ");
        assert_eq!(Snapshot::from_fields(&fields), snapshot);
        let echoed = SnapshotFields {
            state: "N",
            ..SnapshotFields::default()
        };
        assert_eq!(
            Snapshot::from_fields(&echoed).state(),
            Some(UpdateState::ChangesOkNotConfirmed)
        );
        let unknown = SnapshotFields {
            state: "Q",
            ..SnapshotFields::default()
        };
        assert_eq!(Snapshot::from_fields(&unknown).state(), None);
        // audit: COACTUPC-076
        for shown in [format!("{snapshot:?}"), format!("{fields:?}")] {
            assert!(!shown.contains("123456789") && !shown.contains("12345"));
        }
        Ok(())
    }

    #[test]
    fn compare_finds_no_change_in_the_untouched_form() -> TestResult {
        let (snapshot, fetched) = (snapshot(UpdateState::ShowDetails)?, fetched()?);
        let form = Form::shown()?;
        // audit: COACTUPC-040, COACTUPC-041, COACTUPC-043
        assert_eq!(
            compare_old_new(&form.input(), &snapshot, &fetched),
            ChangeCheck::Unchanged
        );
        // audit: COACTUPC-041
        let mut lower = form.clone();
        lower.names[0] = "immanuel".to_owned();
        lower.active_status = "y".to_owned();
        lower.group_id = " gold".to_owned();
        assert_eq!(
            compare_old_new(&lower.input(), &snapshot, &fetched),
            ChangeCheck::Unchanged
        );
        // audit: COACTUPC-044
        let mut same_ssn = form.clone();
        same_ssn.ssn = Some(parts(["123", "45", "6789"]));
        assert_eq!(
            compare_old_new(&same_ssn.input(), &snapshot, &fetched),
            ChangeCheck::Unchanged
        );
        let mut new_ssn = form;
        new_ssn.ssn = Some(parts(["123", "45", "6780"]));
        assert_eq!(
            compare_old_new(&new_ssn.input(), &snapshot, &fetched),
            ChangeCheck::Changed
        );
        Ok(())
    }

    #[test]
    fn compare_counts_every_differing_term() -> TestResult {
        let (snapshot, fetched) = (snapshot(UpdateState::ShowDetails)?, fetched()?);
        let touches: [(&str, Touch); 9] = [
            // audit: COACTUPC-042
            ("blank group ID", |f| f.group_id = String::new()),
            ("blank middle name", |f| f.names[1] = "   ".to_owned()),
            // audit: COACTUPC-040
            ("amount", |f| f.credit_limit = "2020.01".to_owned()),
            ("rejected amount", |f| f.credit_limit = "abc".to_owned()),
            ("open day", |f| f.open[2] = "21".to_owned()),
            // audit: COACTUPC-041
            ("phone prefix", |f| f.phone_1[1] = "556".to_owned()),
            ("EFT", |f| f.eft = "0053581757".to_owned()),
            ("FICO", |f| f.fico = "751".to_owned()),
            ("ZIP", |f| f.zip = "90211".to_owned()),
        ];
        for (name, touch) in touches {
            let mut form = Form::shown()?;
            touch(&mut form);
            assert_eq!(
                compare_old_new(&form.input(), &snapshot, &fetched),
                ChangeCheck::Changed,
                "{name}"
            );
        }
        // audit: COACTUPC-041
        let mut zip_plus_four = customer()?;
        zip_plus_four.addr_zip = text("90210-1234")?;
        let stored = Snapshot::from_records(&account()?, &zip_plus_four);
        assert_eq!(
            compare_old_new(&Form::shown()?.input(), &stored, &fetched),
            ChangeCheck::Changed
        );
        Ok(())
    }

    #[test]
    fn enter_without_a_change_keeps_the_echoed_state() -> TestResult {
        let form = Form::shown()?;
        // audit: COACTUPC-043, COACTUPC-012
        for state in [
            UpdateState::ShowDetails,
            UpdateState::ChangesNotOk,
            UpdateState::ChangesOkNotConfirmed,
        ] {
            let failure = decided(Aid::Enter, &form, state)?
                .err()
                .ok_or("no change was accepted")?;
            assert_eq!(
                shown(&failure),
                input_error(NO_CHANGE, UpdateField::ActiveStatus)
            );
            assert_eq!(failure.state(), state);
        }
        assert_eq!(UpdateState::ShowDetails.info_message(), PRESENTED);
        // audit: COACTUPC-004
        let failure = decided(Aid::Pf5, &form, UpdateState::ShowDetails)?
            .err()
            .ok_or("F5 in state S saved")?;
        assert_eq!(failure.message(), NO_CHANGE);
        Ok(())
    }

    #[test]
    fn enter_with_a_valid_change_validates_it() -> TestResult {
        let mut form = Form::changed()?;
        form.credit_limit = "$3,000.5".to_owned();
        // audit: COACTUPC-013, COACTUPC-014, COACTUPC-078, COACTUPC-082
        for (aid, state) in [
            (Aid::Enter, UpdateState::ShowDetails),
            (Aid::Enter, UpdateState::ChangesNotOk),
            (Aid::Pf5, UpdateState::ShowDetails),
        ] {
            let decision = decided(aid, &form, state)?.map_err(|f| f.message().to_owned())?;
            let Decision::Respond(outcome) = decision else {
                return Err("saved outside state N".to_owned());
            };
            assert_eq!(outcome.state(), UpdateState::ChangesOkNotConfirmed);
            assert_eq!(outcome.info_message(), VALIDATED);
            let shown = outcome.shown_amounts().ok_or("no amounts")?;
            assert_eq!(shown.credit_limit, "+      3,000.50");
            assert!(outcome.state().save_key_shown() && outcome.state().cancel_key_shown());
        }
        Ok(())
    }

    #[test]
    fn enter_in_state_n_runs_no_edit() -> TestResult {
        let mut form = Form::changed()?;
        form.fico = "100".to_owned();
        form.credit_limit = "abc".to_owned();
        // audit: COACTUPC-012, COACTUPC-070
        let decision = decided(Aid::Enter, &form, UpdateState::ChangesOkNotConfirmed)?
            .map_err(|f| f.message().to_owned())?;
        let Decision::Respond(outcome) = decision else {
            return Err("ENTER saved".to_owned());
        };
        assert_eq!(outcome.state(), UpdateState::ChangesOkNotConfirmed);
        let shown = outcome.shown_amounts().ok_or("no amounts")?;
        assert_eq!(shown.credit_limit, "abc            ");
        Ok(())
    }

    #[test]
    fn f5_in_state_n_rechecks_before_saving() -> TestResult {
        // audit: COACTUPC-071, COACTUPC-110
        let save = pending(&Form::changed()?)?;
        assert_eq!(save.account_key(), digits(1)?);
        assert_eq!(save.changes().fico.get(), 750);
        let unchanged = decided(
            Aid::Pf5,
            &Form::shown()?,
            UpdateState::ChangesOkNotConfirmed,
        )?
        .err()
        .ok_or("an unchanged form saved")?;
        assert_eq!(unchanged.message(), NO_CHANGE);
        assert_eq!(unchanged.state(), UpdateState::ShowDetails);
        let mut invalid = Form::changed()?;
        invalid.fico = "100".to_owned();
        let failure = decided(Aid::Pf5, &invalid, UpdateState::ChangesOkNotConfirmed)?
            .err()
            .ok_or("an invalid form saved")?;
        assert_eq!(
            shown(&failure),
            input_error(
                "FICO Score: should be between 300 and 850",
                UpdateField::FicoScore
            )
        );
        assert_eq!(failure.state(), UpdateState::ChangesNotOk);
        Ok(())
    }

    #[test]
    fn finished_states_restart_and_unknown_states_abend() -> TestResult {
        // audit: COACTUPC-006
        for state in [
            UpdateState::NotFetched,
            UpdateState::ChangesOkayedAndDone,
            UpdateState::LockError,
            UpdateState::ChangesFailed,
        ] {
            let decision =
                decided(Aid::Pf5, &Form::changed()?, state)?.map_err(|f| f.message().to_owned())?;
            let Decision::Respond(outcome) = decision else {
                return Err(format!("saved in state {state:?}"));
            };
            assert_eq!(outcome, UpdateOutcome::Restart);
            assert_eq!(outcome.info_message(), SEARCH);
            assert!(outcome.shown_amounts().is_none());
        }
        // The `WHEN OTHER` of 2000-DECIDE-ACTION, reached only by a code the program never sets.
        let echoed = SnapshotFields {
            state: "Q".to_owned(),
            ..snapshot(UpdateState::ShowDetails)?.fields()
        };
        let unknown = Snapshot::from_fields(&echoed);
        let form = Form::shown()?;
        let failure = decide_with(
            Aid::Enter,
            &form.input(),
            &unknown,
            &fetched()?,
            &clock()?,
            accept_all,
        )
        .err()
        .ok_or("an unknown state answered")?;
        assert_eq!(failure.message(), "UNEXPECTED DATA SCENARIO");
        assert_eq!(failure.class(), FailureClass::Abend);
        assert_eq!(failure.class().http_status(), 500);
        let abend = failure.abend_data().ok_or("no abend data")?;
        assert_eq!(abend.code.as_bytes(), b"0001");
        assert_eq!(abend.culprit.as_bytes(), b"COACTUPC");
        let changed = Form::changed()?;
        let decision = decide_with(
            Aid::Enter,
            &changed.input(),
            &unknown,
            &fetched()?,
            &clock()?,
            accept_all,
        )
        .map_err(|f| f.message().to_owned())?;
        assert!(matches!(
            decision,
            Decision::Respond(UpdateOutcome::ChangesValidated { .. })
        ));
        Ok(())
    }

    /// Runs each case: the change applied to the changed form, the message and field of the
    /// first failing edit.
    fn assert_edit_failures(cases: &[(Touch, &str, UpdateField)]) -> TestResult {
        for &(touch, message, field) in cases {
            let mut form = Form::changed()?;
            touch(&mut form);
            assert_eq!(edit_failure(&form)?, input_error(message, field));
        }
        Ok(())
    }

    #[test]
    fn account_edits_return_their_messages() -> TestResult {
        use UpdateField::{
            ActiveStatus, CashCreditLimit, CreditLimit, CurrentBalance, CurrentCycleCredit,
            CurrentCycleDebit, ExpiryMonth, OpenMonth, OpenYear, ReissueDay,
        };
        let supplied = "Account Status must be supplied.";
        assert_edit_failures(&[
            // audit: COACTUPC-015, COACTUPC-049
            (|f| f.active_status = String::new(), supplied, ActiveStatus),
            (|f| f.active_status = "0".to_owned(), supplied, ActiveStatus),
            (
                |f| f.active_status = "y".to_owned(),
                "Account Status must be Y or N.",
                ActiveStatus,
            ),
            // audit: COACTUPC-016, COACTUPC-118
            (
                |f| f.open[0] = String::new(),
                "Open Date : Year must be supplied.",
                OpenYear,
            ),
            // audit: COACTUPC-120
            (
                |f| f.open[0] = "1800".to_owned(),
                "Open Date : Century is not valid.",
                OpenYear,
            ),
            // audit: COACTUPC-122
            (
                |f| f.open[1] = "13".to_owned(),
                "Open Date: Month must be a number between 1 and 12.",
                OpenMonth,
            ),
            // audit: COACTUPC-126
            (
                |f| f.open = parts(["2014", "04", "31"]),
                "Open Date:Cannot have 31 days in this month.",
                OpenMonth,
            ),
            // audit: COACTUPC-128
            (
                |f| f.open = parts(["1900", "02", "29"]),
                "Open Date:Not a leap year.Cannot have 29 days in this month.",
                OpenYear,
            ),
            // audit: COACTUPC-017, COACTUPC-054
            (
                |f| f.credit_limit = String::new(),
                "Credit Limit must be supplied.",
                CreditLimit,
            ),
            (
                |f| f.credit_limit = "1.2.3".to_owned(),
                "Credit Limit is not valid",
                CreditLimit,
            ),
            (
                |f| f.credit_limit = "1,".to_owned(),
                "Credit Limit is not valid",
                CreditLimit,
            ),
            // audit: COACTUPC-018, COACTUPC-121
            (
                |f| f.expiry[1] = String::new(),
                "Expiry Date : Month must be supplied.",
                ExpiryMonth,
            ),
            // audit: COACTUPC-019
            (
                |f| f.cash_credit_limit = "12 34".to_owned(),
                "Cash Credit Limit is not valid",
                CashCreditLimit,
            ),
            // audit: COACTUPC-020, COACTUPC-125
            (
                |f| f.reissue[2] = "32".to_owned(),
                "Reissue Date:day must be a number between 1 and 31.",
                ReissueDay,
            ),
            // audit: COACTUPC-021
            (
                |f| f.current_balance = "*".to_owned(),
                "Current Balance must be supplied.",
                CurrentBalance,
            ),
            // audit: COACTUPC-022
            (
                |f| f.current_cycle_credit = String::new(),
                "Current Cycle Credit Limi must be supplied.",
                CurrentCycleCredit,
            ),
            // audit: COACTUPC-023
            (
                |f| f.current_cycle_debit = "x".to_owned(),
                "Current Cycle Debit Limit is not valid",
                CurrentCycleDebit,
            ),
        ])
    }

    #[test]
    fn customer_edits_return_their_messages() -> TestResult {
        use UpdateField::{DobDay, DobYear, FicoScore, SsnPart1, SsnPart2, SsnPart3};
        let ssn_range = "SSN: First 3 chars: should not be 000, 666, or between 900 and 999";
        assert_edit_failures(&[
            // audit: COACTUPC-024, COACTUPC-060
            (
                |f| f.ssn = Some(parts(["", "45", "6789"])),
                "SSN: First 3 chars must be supplied.",
                SsnPart1,
            ),
            // audit: COACTUPC-061
            (
                |f| f.ssn = Some(parts(["666", "45", "6789"])),
                ssn_range,
                SsnPart1,
            ),
            (
                |f| f.ssn = Some(parts(["950", "00", "6789"])),
                ssn_range,
                SsnPart1,
            ),
            // audit: COACTUPC-062
            (
                |f| f.ssn = Some(parts(["123", "00", "6789"])),
                "SSN 4th & 5th chars must not be zero.",
                SsnPart2,
            ),
            // audit: COACTUPC-063
            (
                |f| f.ssn = Some(parts(["123", "45", "67a9"])),
                "SSN Last 4 chars must be all numeric.",
                SsnPart3,
            ),
            // audit: COACTUPC-025, COACTUPC-124
            (
                |f| f.dob[2] = String::new(),
                "Date of Birth : Day must be supplied.",
                DobDay,
            ),
            // audit: COACTUPC-130
            (
                |f| f.dob = parts(["2022", "07", "18"]),
                "Date of Birth:cannot be in the future",
                DobYear,
            ),
            // audit: COACTUPC-026, COACTUPC-053
            (
                |f| f.fico = String::new(),
                "FICO Score must be supplied.",
                FicoScore,
            ),
            (
                |f| f.fico = "7a0".to_owned(),
                "FICO Score must be all numeric.",
                FicoScore,
            ),
            (
                |f| f.fico = "000".to_owned(),
                "FICO Score must not be zero.",
                FicoScore,
            ),
            // audit: COACTUPC-066
            (
                |f| f.fico = "851".to_owned(),
                "FICO Score: should be between 300 and 850",
                FicoScore,
            ),
        ])
    }

    #[test]
    fn name_and_address_edits_return_their_messages() -> TestResult {
        use UpdateField::{AddressLine1, City, Country, FirstName, LastName, MiddleName, State};
        assert_edit_failures(&[
            // audit: COACTUPC-027, COACTUPC-050
            (
                |f| f.names[0] = String::new(),
                "First Name must be supplied.",
                FirstName,
            ),
            (
                |f| f.names[0] = "J0hn".to_owned(),
                "First Name can have alphabets only.",
                FirstName,
            ),
            // audit: COACTUPC-028, COACTUPC-052
            (
                |f| f.names[1] = "M.".to_owned(),
                "Middle Name can have alphabets only.",
                MiddleName,
            ),
            // audit: COACTUPC-029
            (
                |f| f.names[2] = String::new(),
                "Last Name must be supplied.",
                LastName,
            ),
            // audit: COACTUPC-030, COACTUPC-048
            (
                |f| f.address_line_1 = "   ".to_owned(),
                "Address Line 1 must be supplied.",
                AddressLine1,
            ),
            // audit: COACTUPC-031, COACTUPC-065
            (
                |f| f.state = String::new(),
                "State must be supplied.",
                State,
            ),
            (
                |f| f.state = "C1".to_owned(),
                "State can have alphabets only.",
                State,
            ),
            (
                |f| f.state = "ZZ".to_owned(),
                "State: is not a valid state code",
                State,
            ),
            (
                |f| f.state = "ca".to_owned(),
                "State: is not a valid state code",
                State,
            ),
            // audit: COACTUPC-033
            (
                |f| f.city = "City 9".to_owned(),
                "City can have alphabets only.",
                City,
            ),
            // audit: COACTUPC-034
            (
                |f| f.country = String::new(),
                "Country must be supplied.",
                Country,
            ),
        ])
    }

    #[test]
    fn zip_phone_and_flag_edits_return_their_messages() -> TestResult {
        use UpdateField::{
            EftAccountId, Phone1Area, Phone2Line, Phone2Prefix, PrimaryCardHolder, State, Zip,
        };
        assert_edit_failures(&[
            // audit: COACTUPC-032
            (|f| f.zip = String::new(), "Zip must be supplied.", Zip),
            (
                |f| f.zip = "9021A".to_owned(),
                "Zip must be all numeric.",
                Zip,
            ),
            (|f| f.zip = "00000".to_owned(), "Zip must not be zero.", Zip),
            // audit: COACTUPC-035, COACTUPC-056
            (
                |f| f.phone_1[0] = "000".to_owned(),
                "Phone Number 1: Area code cannot be zero",
                Phone1Area,
            ),
            (
                |f| f.phone_1[0] = "12a".to_owned(),
                "Phone Number 1: Area code must be A 3 digit number.",
                Phone1Area,
            ),
            // audit: COACTUPC-057
            (
                |f| f.phone_1[0] = "111".to_owned(),
                "Phone Number 1: Not valid North America general purpose area code",
                Phone1Area,
            ),
            // audit: COACTUPC-036, COACTUPC-058
            (
                |f| f.phone_2[1] = String::new(),
                "Phone Number 2: Prefix code must be supplied.",
                Phone2Prefix,
            ),
            // audit: COACTUPC-059
            (
                |f| f.phone_2[2] = "0000".to_owned(),
                "Phone Number 2: Line number code cannot be zero",
                Phone2Line,
            ),
            // audit: COACTUPC-037
            (
                |f| f.eft = "123".to_owned(),
                "EFT Account Id must be all numeric.",
                EftAccountId,
            ),
            (
                |f| f.eft = "0000000000".to_owned(),
                "EFT Account Id must not be zero.",
                EftAccountId,
            ),
            // audit: COACTUPC-038
            (
                |f| f.primary = String::new(),
                "Primary Card Holder must be supplied.",
                PrimaryCardHolder,
            ),
            (
                |f| f.primary = "X".to_owned(),
                "Primary Card Holder must be Y or N.",
                PrimaryCardHolder,
            ),
            // audit: COACTUPC-067
            (
                |f| f.zip = "10001".to_owned(),
                "Invalid zip code for state",
                State,
            ),
        ])
    }

    #[test]
    fn the_first_failing_edit_sets_the_message() -> TestResult {
        // audit: COACTUPC-013
        let mut form = Form::changed()?;
        form.active_status = "y".to_owned();
        form.fico = "100".to_owned();
        form.zip = String::new();
        assert_eq!(
            edit_failure(&form)?,
            input_error("Account Status must be Y or N.", UpdateField::ActiveStatus)
        );
        // audit: COACTUPC-067
        let mut both = Form::changed()?;
        both.zip = "10001".to_owned();
        both.primary = "X".to_owned();
        assert_eq!(
            edit_failure(&both)?,
            input_error(
                "Primary Card Holder must be Y or N.",
                UpdateField::PrimaryCardHolder
            )
        );
        // audit: COACTUPC-078
        let mut amounts = Form::changed()?;
        amounts.credit_limit = "1,000".to_owned();
        amounts.cash_credit_limit = "12 34".to_owned();
        let failure = edited(&amounts)?.err().ok_or("the edits passed")?;
        let shown = failure.shown_amounts().ok_or("no amounts")?;
        assert_eq!(shown.credit_limit, "+      1,000.00");
        assert_eq!(shown.cash_credit_limit, "12 34          ");
        assert_eq!(failure.state().info_message(), PRESENTED);
        Ok(())
    }

    #[test]
    fn unedited_and_optional_fields_pass() -> TestResult {
        let mut form = Form::changed()?;
        // audit: COACTUPC-028, COACTUPC-055
        form.names[1] = String::new();
        form.phone_2 = parts(["", "", ""]);
        // audit: COACTUPC-039
        form.address_line_2 = "#12 & co!".to_owned();
        form.government_id = String::new();
        form.group_id = "g-1".to_owned();
        form.customer_id = "abc".to_owned();
        // audit: COACTUPC-008, COACTUPC-123
        form.expiry = parts(["2025", "05", "20"]);
        let changes = edited(&form)?.map_err(|f| f.message().to_owned())?;
        assert_eq!(changes.expiry_date(), date(2025, 5, 20)?);
        assert_eq!(changes.open_date(), date(2014, 11, 20)?);
        assert_eq!(changes.reissue_date(), date(2025, 5, 20)?);
        assert_eq!(changes.dob(), date(1961, 6, 8)?);
        // audit: COACTUPC-009
        assert_eq!(
            changes.amounts(),
            [
                money(202_000)?,
                money(102_000)?,
                money(19_400)?,
                money(0)?,
                money(-5_025)?
            ]
        );
        Ok(())
    }

    #[test]
    fn the_le_step_follows_its_verdict() -> TestResult {
        // HALTED(H5), BLOCKED: these substitute the CEEDAYS verdict; they prove only that the
        // edit routes it, never which dates Language Environment accepts.
        // audit: COACTUPC-129
        let form = Form::changed()?;
        let rejected = edit_with(&form.input(), &fetched()?, &clock()?, reject_all)
            .err()
            .ok_or("a rejected date passed")?;
        assert_eq!(rejected.field(), Some(UpdateField::OpenYear));
        assert!(
            rejected
                .message()
                .starts_with("Open Date validation error Sev code: ")
        );
        assert_eq!(rejected.class().http_status(), 400);
        let accepted = edit_with(&form.input(), &fetched()?, &clock()?, accept_all);
        assert!(accepted.is_ok());
        Ok(())
    }

    #[test]
    fn numval_c_reads_the_audited_forms() {
        // audit: COACTUPC-009, COACTUPC-054
        let cases = [
            ("1234.56", Some(Decimal::new(123_456, 2))),
            ("1,234.56", Some(Decimal::new(123_456, 2))),
            ("$1,234.56", Some(Decimal::new(123_456, 2))),
            ("  -50 ", Some(Decimal::new(-50, 0))),
            ("- $ 50", Some(Decimal::new(-50, 0))),
            ("50-", Some(Decimal::new(-50, 0))),
            ("50CR", Some(Decimal::new(-50, 0))),
            ("50 db", Some(Decimal::new(-50, 0))),
            ("+7", Some(Decimal::new(7, 0))),
            (".5", Some(Decimal::new(5, 1))),
            ("     -1,940.00", Some(Decimal::new(-194_000, 2))),
            ("1,23,4", Some(Decimal::new(1234, 0))),
            ("1,234.", Some(Decimal::new(1234, 0))),
            ("1,", None),
            ("1,,2", None),
            ("1,.00", None),
            ("1, ", None),
            ("1,-", None),
            ("1,CR", None),
            ("$1,", None),
            ("-1,", None),
            ("1.2.3", None),
            ("12 34", None),
            ("abc", None),
            ("50C", None),
            ("-50-", None),
            ("", None),
            (",5", None),
            ("1.5,0", None),
            ("€5", None),
        ];
        for (text, expected) in cases {
            assert_eq!(numval_c(text), expected, "{text:?}");
        }
    }

    #[test]
    fn a_ten_digit_amount_loses_its_high_digit_on_the_screen() -> TestResult {
        // audit: COACTUPC-109
        let mut form = Form::changed()?;
        form.credit_limit = "1234567890.12".to_owned();
        let decision = decided(Aid::Enter, &form, UpdateState::ShowDetails)?
            .map_err(|f| f.message().to_owned())?;
        let Decision::Respond(outcome) = decision else {
            return Err("ENTER saved".to_owned());
        };
        let shown = outcome.shown_amounts().ok_or("no amounts")?;
        assert_eq!(shown.credit_limit, "+234,567,890.12");
        let mut resent = form;
        resent.credit_limit.clone_from(&shown.credit_limit);
        let save = pending(&resent)?;
        assert_eq!(save.changes().credit_limit, money(23_456_789_012)?);
        Ok(())
    }

    #[test]
    fn the_save_rewrites_the_account_as_9600_builds_it() -> TestResult {
        let mut form = Form::changed()?;
        form.active_status = "N".to_owned();
        form.credit_limit = "5,000.00".to_owned();
        form.reissue = parts(["2026", "01", "31"]);
        form.group_id = "PLAT".to_owned();
        let stored = account()?;
        // audit: COACTUPC-103, COACTUPC-109, COACTUPC-111
        let (rewritten, _) = effects(&form)?
            .map_err(|f| f.message().to_owned())?
            .into_parts();
        assert_eq!(rewritten.id, stored.id);
        assert_eq!(rewritten.active_status.as_bytes(), b"N");
        assert_eq!(rewritten.credit_limit, money(500_000)?);
        assert_eq!(rewritten.curr_cyc_debit, stored.curr_cyc_debit);
        assert_eq!(rewritten.reissue_date, pic_date(2026, 1, 31)?);
        assert_eq!(rewritten.open_date, stored.open_date);
        assert_eq!(rewritten.addr_zip.as_bytes(), b"PLAT      ");
        assert_eq!(rewritten.group_id, stored.group_id);
        assert_eq!(rewritten.filler, stored.filler);
        // audit: COACTUPC-105
        let mut blank_group = Form::changed()?;
        blank_group.group_id = String::new();
        let effects = effects(&blank_group)?.map_err(|f| f.message().to_owned())?;
        assert!(effects.account().addr_zip.is_spaces());
        Ok(())
    }

    #[test]
    fn the_save_rewrites_the_customer_as_9600_builds_it() -> TestResult {
        let mut form = Form::changed()?;
        form.names[0] = "immanuel".to_owned();
        form.phone_1 = parts(["202", "555", "0001"]);
        form.dob = parts(["1970", "01", "02"]);
        form.fico = "640".to_owned();
        let stored = customer()?;
        // audit: COACTUPC-104, COACTUPC-111
        let save = effects(&form)?.map_err(|f| f.message().to_owned())?;
        let CustomerRewrite::Record(rewritten) = save.customer() else {
            return Err("no customer record".to_owned());
        };
        assert_eq!(rewritten.id, stored.id);
        assert_eq!(
            rewritten.first_name.as_bytes(),
            text::<25>("immanuel")?.as_bytes()
        );
        assert_eq!(rewritten.last_name, text::<25>("Kessling")?);
        assert_eq!(rewritten.addr_zip.as_bytes(), b"90210     ");
        assert_eq!(rewritten.phone_num_1.as_bytes(), b"(202)555-0001  ");
        assert_eq!(rewritten.dob_yyyy_mm_dd, pic_date(1970, 1, 2)?);
        assert_eq!(rewritten.fico_credit_score, digits(640)?);
        assert_eq!(rewritten.filler, stored.filler);
        // audit: COACTUPC-064
        assert_eq!(rewritten.ssn.expose().get(), SSN);
        let mut resent = form;
        resent.ssn = Some(parts(["234", "56", "7890"]));
        let save = effects(&resent)?.map_err(|f| f.message().to_owned())?;
        let CustomerRewrite::Record(replaced) = save.customer() else {
            return Err("no customer record".to_owned());
        };
        assert_eq!(replaced.ssn.expose().get(), 234_567_890);
        Ok(())
    }

    #[test]
    fn blank_optional_fields_are_written_as_spaces() -> TestResult {
        let mut form = Form::changed()?;
        form.names[1] = String::new();
        form.address_line_2 = "*".to_owned();
        form.government_id = String::new();
        form.phone_2 = parts(["", "", ""]);
        // audit: COACTUPC-105
        let save = effects(&form)?.map_err(|f| f.message().to_owned())?;
        let CustomerRewrite::Record(rewritten) = save.customer() else {
            return Err("no customer record".to_owned());
        };
        assert!(rewritten.middle_name.is_spaces());
        assert!(rewritten.addr_line_2.is_spaces());
        assert!(rewritten.govt_issued_id.is_spaces());
        assert_eq!(rewritten.phone_num_2.as_bytes(), b"(   )   -      ");
        Ok(())
    }

    #[test]
    fn an_unwritable_save_fails_before_any_write() -> TestResult {
        // audit: COACTUPC-111
        let mut form = Form::changed()?;
        form.open[2] = "1.".to_owned();
        let save = pending(&form)?;
        assert_eq!(save.changes().open_date(), date(2014, 11, 1)?);
        let failure = save
            .save_effects(&account()?, &customer()?)
            .err()
            .ok_or("a non-digit day was written")?;
        assert_eq!(shown(&failure), (UPDATE_FAILED.to_owned(), None, 500));
        assert_eq!(failure.state(), UpdateState::ChangesFailed);
        // audit: COACTUPC-112
        let mut other_key = Form::changed()?;
        other_key.customer_id = "000000002".to_owned();
        let save = effects(&other_key)?.map_err(|f| f.message().to_owned())?;
        assert_eq!(save.customer(), &CustomerRewrite::KeyChanged);
        assert_eq!(save.account().id, digits(1)?);
        let mut other_account = Form::changed()?;
        other_account.account_id = "00000000002".to_owned();
        let failure = pending(&other_account)?
            .save_effects(&account()?, &customer()?)
            .err()
            .ok_or("another account was rewritten")?;
        assert_eq!(failure.message(), UPDATE_FAILED);
        Ok(())
    }

    #[test]
    fn the_save_outcomes_set_their_states() -> TestResult {
        let save = pending(&Form::changed()?)?;
        // audit: COACTUPC-101
        let lock = save.account_lock_failed();
        assert_eq!(shown(&lock), (LOCK_ACCOUNT.to_owned(), None, 500));
        assert_eq!(lock.state(), UpdateState::LockError);
        assert_eq!(lock.state().info_message(), UNSUCCESSFUL);
        assert!(lock.state().cancel_key_shown() && !lock.state().save_key_shown());
        // audit: COACTUPC-102
        let lock = save.customer_lock_failed();
        assert_eq!(shown(&lock), (LOCK_CUSTOMER.to_owned(), None, 500));
        assert_eq!(lock.state(), UpdateState::ChangesOkayedAndDone);
        assert_eq!(lock.state().info_message(), COMMITTED);
        // audit: COACTUPC-106, COACTUPC-107
        let rewrite = save.rewrite_failed();
        assert_eq!(shown(&rewrite), (UPDATE_FAILED.to_owned(), None, 500));
        assert_eq!(rewrite.state(), UpdateState::ChangesFailed);
        assert_eq!(rewrite.state().info_message(), UNSUCCESSFUL);
        // audit: COACTUPC-108, COACTUPC-082
        let done = save.committed();
        assert_eq!(done.state(), UpdateState::ChangesOkayedAndDone);
        assert_eq!(done.info_message(), COMMITTED);
        assert!(!done.state().save_key_shown() && !done.state().cancel_key_shown());
        let shown = done.shown_amounts().ok_or("no amounts")?;
        assert_eq!(shown.credit_limit, "+      2,020.00");
        for failure in [&lock, &rewrite] {
            assert!(failure.shown_amounts().is_some());
        }
        Ok(())
    }

    #[test]
    fn check_change_in_rec_passes_an_untouched_record() -> TestResult {
        let (account, customer) = (account()?, customer()?);
        let snapshot = snapshot(UpdateState::ChangesOkNotConfirmed)?;
        // audit: COACTUPC-113, COACTUPC-114
        assert_eq!(
            check_change_in_rec(&snapshot, (&account, &customer)),
            Ok(())
        );
        // The group ID is compared after LOWER-CASE, the names after UPPER-CASE.
        let mut group = account.clone();
        group.group_id = text("gold")?;
        let mut name = customer.clone();
        name.first_name = text("IMMANUEL")?;
        assert_eq!(check_change_in_rec(&snapshot, (&group, &name)), Ok(()));
        // audit: COACTUPC-115
        let mut first_five = customer;
        first_five.ssn = Ssn::new(digits(987_656_789)?);
        assert_eq!(
            check_change_in_rec(&snapshot, (&account, &first_five)),
            Ok(())
        );
        Ok(())
    }

    #[test]
    fn check_change_in_rec_reports_a_change_by_another_user() -> TestResult {
        type AccountTouch = fn(&mut AccountRecord) -> TestResult;
        type CustomerTouch = fn(&mut CustomerRecord) -> TestResult;
        let snapshot = snapshot(UpdateState::ChangesOkNotConfirmed)?;
        let account_touches: [AccountTouch; 3] = [
            // audit: COACTUPC-113
            |a| {
                a.active_status = text("y")?;
                Ok(())
            },
            |a| {
                a.curr_bal = money(19_401)?;
                Ok(())
            },
            |a| {
                a.expiraion_date = pic_date(2025, 5, 21)?;
                Ok(())
            },
        ];
        let customer_touches: [CustomerTouch; 4] = [
            // audit: COACTUPC-114
            |c| {
                c.addr_zip = text("90210-1234")?;
                Ok(())
            },
            |c| {
                c.phone_num_2 = text("(202)555-9875")?;
                Ok(())
            },
            |c| {
                c.eft_account_id = text("0053581757")?;
                Ok(())
            },
            // audit: COACTUPC-115
            |c| {
                c.ssn = Ssn::new(digits(SSN + 1)?);
                Ok(())
            },
        ];
        for touch in account_touches {
            let mut account = account()?;
            touch(&mut account)?;
            let failure = check_change_in_rec(&snapshot, (&account, &customer()?))
                .err()
                .ok_or("an account change passed")?;
            assert_eq!(shown(&failure), (CHANGED.to_owned(), None, 409));
            // audit: COACTUPC-116
            assert_eq!(failure.state(), UpdateState::ShowDetails);
        }
        for touch in customer_touches {
            let mut customer = customer()?;
            touch(&mut customer)?;
            let failure = check_change_in_rec(&snapshot, (&account()?, &customer))
                .err()
                .ok_or("a customer change passed")?;
            assert_eq!(failure.class(), FailureClass::Changed);
        }
        let echoed = SnapshotFields {
            credit_limit: "not an amount".to_owned(),
            ..snapshot.fields()
        };
        let unknown = Snapshot::from_fields(&echoed);
        assert!(check_change_in_rec(&unknown, (&account()?, &customer()?)).is_err());
        Ok(())
    }

    #[test]
    fn no_response_or_debug_text_holds_the_ssn() -> TestResult {
        // audit: COACTUPC-076
        let mut form = Form::changed()?;
        form.ssn = Some(parts(["234", "56", "7890"]));
        let save = pending(&form)?;
        let failure = decided(Aid::Enter, &Form::shown()?, UpdateState::ShowDetails)?
            .err()
            .ok_or("no change was accepted")?;
        let effects = save
            .save_effects(&account()?, &customer()?)
            .map_err(|f| f.message().to_owned())?;
        let texts = [
            format!("{save:?}"),
            format!("{failure:?}"),
            format!("{effects:?}"),
            format!("{:?}", form.input()),
            format!("{:?}", fetched()?),
            format!("{:?}", save.committed()),
        ];
        for text in texts {
            for secret in ["234567890", "23456", "123456789", "12345"] {
                assert!(!text.contains(secret), "{secret} in {text}");
            }
        }
        Ok(())
    }

    #[test]
    fn states_and_fields_name_their_cobol_values() -> TestResult {
        use UpdateField::{
            AccountId, ActiveStatus, AddressLine1, CashCreditLimit, City, Country, CreditLimit,
            CurrentBalance, CurrentCycleCredit, CurrentCycleDebit, DobDay, DobMonth, DobYear,
            EftAccountId, ExpiryDay, ExpiryMonth, ExpiryYear, FicoScore, FirstName, LastName,
            MiddleName, OpenDay, OpenMonth, OpenYear, Phone1Area, Phone1Line, Phone1Prefix,
            Phone2Area, Phone2Line, Phone2Prefix, PrimaryCardHolder, ReissueDay, ReissueMonth,
            ReissueYear, SsnPart1, SsnPart2, SsnPart3, State, Zip,
        };
        // audit: COACTUPC-079, COACTUPC-100
        for (state, code) in [
            (UpdateState::NotFetched, ' '),
            (UpdateState::ShowDetails, 'S'),
            (UpdateState::ChangesNotOk, 'E'),
            (UpdateState::ChangesOkNotConfirmed, 'N'),
            (UpdateState::ChangesOkayedAndDone, 'C'),
            (UpdateState::LockError, 'L'),
            (UpdateState::ChangesFailed, 'F'),
        ] {
            assert_eq!(state.code(), code);
            assert_eq!(UpdateState::from_code(&code.to_string()), Some(state));
        }
        assert_eq!(UpdateState::from_code(""), Some(UpdateState::NotFetched));
        assert_eq!(UpdateState::ChangesNotOk.info_message(), PRESENTED);
        // audit: COACTUPC-081
        let fields = [
            AccountId,
            ActiveStatus,
            OpenYear,
            OpenMonth,
            OpenDay,
            CreditLimit,
            ExpiryYear,
            ExpiryMonth,
            ExpiryDay,
            CashCreditLimit,
            ReissueYear,
            ReissueMonth,
            ReissueDay,
            CurrentBalance,
            CurrentCycleCredit,
            CurrentCycleDebit,
            SsnPart1,
            SsnPart2,
            SsnPart3,
            DobYear,
            DobMonth,
            DobDay,
            FicoScore,
            FirstName,
            MiddleName,
            LastName,
            AddressLine1,
            State,
            Zip,
            City,
            Country,
            Phone1Area,
            Phone1Prefix,
            Phone1Line,
            Phone2Area,
            Phone2Prefix,
            Phone2Line,
            EftAccountId,
            PrimaryCardHolder,
        ];
        let mut names: Vec<&str> = fields.iter().map(|field| field.map_name()).collect();
        assert!(names.iter().all(|name| (6..=7).contains(&name.len())));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), fields.len());
        assert_eq!(ActiveStatus.map_name(), "ACSTTUS");
        assert_eq!(City.map_name(), "ACSCITY");
        let failure = account_key("").err().ok_or("blank passed")?;
        assert_eq!(failure.to_string(), NO_INPUT);
        Ok(())
    }

    #[test]
    fn a_blank_date_sent_blank_is_still_a_change() -> TestResult {
        // audit: COACTUPC-042
        let mut stored = account()?;
        stored.reissue_date = PicDate::BLANK;
        let snapshot = Snapshot::from_records(&stored, &customer()?);
        let mut form = Form::shown()?;
        form.reissue = parts(["", "", ""]);
        assert_eq!(
            compare_old_new(&form.input(), &snapshot, &fetched()?),
            ChangeCheck::Changed
        );
        Ok(())
    }

    #[test]
    fn a_save_key_that_cannot_be_read_is_a_lock_failure() -> TestResult {
        // audit: COACTUPC-101
        let mut form = Form::changed()?;
        form.account_id = "ABC".to_owned();
        let failure = decided(Aid::Pf5, &form, UpdateState::ChangesOkNotConfirmed)?
            .err()
            .ok_or("an unreadable key saved")?;
        assert_eq!(shown(&failure), (LOCK_ACCOUNT.to_owned(), None, 500));
        assert_eq!(failure.state(), UpdateState::LockError);
        Ok(())
    }

    #[test]
    fn a_customer_field_no_column_can_hold_fails_the_save() -> TestResult {
        // audit: COACTUPC-039, COACTUPC-111
        let mut form = Form::changed()?;
        form.government_id = "ID\u{7}".to_owned();
        let failure = effects(&form)?
            .err()
            .ok_or("a control character was written")?;
        assert_eq!(shown(&failure), (UPDATE_FAILED.to_owned(), None, 500));
        Ok(())
    }

    #[test]
    fn the_stand_in_verdict_entry_points_match_their_with_forms() -> TestResult {
        // HALTED(H5), BLOCKED: `edit` and `decide` pass CSUTLDTC's stand-in verdict; this pins
        // only that they equal the `_with` forms given that verdict, never which dates pass.
        // audit: COACTUPC-129
        let form = Form::changed()?;
        let (fetched, clock) = (fetched()?, clock()?);
        let plain = edit(&form.input(), &fetched, &clock);
        let with = edit_with(&form.input(), &fetched, &clock, validate);
        assert_eq!(
            plain.as_ref().err().map(shown),
            with.as_ref().err().map(shown)
        );
        assert_eq!(plain.is_ok(), with.is_ok());
        let snapshot = snapshot(UpdateState::ShowDetails)?;
        let plain = decide(Aid::Enter, &form.input(), &snapshot, &fetched, &clock);
        let with = decide_with(
            Aid::Enter,
            &form.input(),
            &snapshot,
            &fetched,
            &clock,
            validate,
        );
        assert_eq!(
            plain.as_ref().err().map(shown),
            with.as_ref().err().map(shown)
        );
        assert_eq!(plain.is_ok(), with.is_ok());
        Ok(())
    }
}
