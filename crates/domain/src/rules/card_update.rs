//! `COCRDUPC` (CCUP), the credit card update: `PUT /cards/{id}`.
//!
//! The 3270 dialogue takes three turns: the keys and the fetch
//! (`1210-EDIT-ACCOUNT`, `1220-EDIT-CARD`, `9000-READ-DATA`), the ENTER turn
//! that compares and edits the changes (`1200-EDIT-MAP-INPUTS`), and the PF5
//! turn that writes them (`9200-WRITE-PROCESSING`). One request performs all
//! three (E-56), in the order the validation audit ranks their messages
//! (V-COCRDUPC-002 to V-COCRDUPC-020): [`update`] runs [`edit_keys`],
//! [`read_data`], the card-field edits of [`validate`] and [`apply`] in that
//! order, editing the keys once, and each step is public for an adapter that
//! sequences them itself.
//!
//! Between turns the program keeps the fetched values, `CCUP-OLD-DETAILS`, in
//! its COMMAREA (P-COCRDUPC-008). The stateless API keeps a
//! [`VersionToken`] instead (E-24): [`record_version`] digests the fields
//! `9300-CHECK-CHANGE-IN-REC` compares, keyed with the server's
//! [`VersionKey`], the `GET /cards/{id}` response carries it, and [`apply`]
//! recomputes it under the same key from the record it locks. The key keeps
//! the token from disclosing the verification code it digests. A request
//! without a token is the API's 400 (V-COCRDUPC-001) and never reaches this
//! module.
//!
//! # Outcomes
//!
//! | Outcome | Kind | BMS field | Rows |
//! |---|---|---|---|
//! | Key blank, zeros or not numeric; both keys blank | `Empty`, `Invalid` | `ACCTSID`, `CARDSID` | V-COCRDUPC-002 to V-COCRDUPC-006 |
//! | Card not on file; card read failed | `NotFound`, `Unexpected` | `ACCTSID` | V-COCRDUPC-007, V-COCRDUPC-008 |
//! | No change detected | none: [`CardEdit::NoChange`], 200 | — | V-COCRDUPC-009 |
//! | Name, status, month or year edit failed | `Empty`, `Invalid` | `CRDNAME`, `CRDSTCD`, `EXPMON`, `EXPYEAR` | V-COCRDUPC-010 to V-COCRDUPC-017 |
//! | Lock failed; rewrite failed | `Unexpected` | `ACCTSID` | V-COCRDUPC-018, V-COCRDUPC-020 |
//! | Record changed since it was fetched | `Conflict` | `CRDNAME` | V-COCRDUPC-019 |
//! | Rewritten | none: [`CardUpdated`], 200 | — | P-COCRDUPC-007 |
//!
//! Every edit of a step runs, and the first failing edit in COBOL order
//! supplies the message: the `IF WS-RETURN-MSG-OFF` guard of each edit
//! paragraph [app/cbl/COCRDUPC.cbl:L173-174]. The rules keep that order by
//! returning the first failure of the edits they run, which also yields the
//! error kind each audit row assigns; [`ReturnMsg`](super::ReturnMsg) keeps
//! only the text and the field, so it is not used here.
//!
//! The card's verification code is never written from the request (E-75,
//! P-COCRDUPC-028): every update keeps the stored `card_cvv_cd`.

use std::fmt;
use std::ops::RangeInclusive;

use super::{VersionKey, VersionToken, version_token};
use crate::entities::Card;
use crate::error::{DomainError, ErrorKind};
use crate::messages;
use crate::numeric::{AccountId, Alnum, CardNumber};
use crate::ports::{CardReader, CardWriter, PortError};

/// `ACCTSID`, the account number of the `COCRDUP` map.
const FIELD_ACCTSID: &str = "ACCTSID";

/// `CARDSID`, the card number of the `COCRDUP` map.
const FIELD_CARDSID: &str = "CARDSID";

/// `CRDNAME`, the embossed name of the `COCRDUP` map.
const FIELD_CRDNAME: &str = "CRDNAME";

/// `CRDSTCD`, the active status of the `COCRDUP` map.
const FIELD_CRDSTCD: &str = "CRDSTCD";

/// `EXPMON`, the expiry month of the `COCRDUP` map.
const FIELD_EXPMON: &str = "EXPMON";

/// `EXPYEAR`, the expiry year of the `COCRDUP` map.
const FIELD_EXPYEAR: &str = "EXPYEAR";

/// `ERROR-OPNAME` of a failed card read [app/cbl/COCRDUPC.cbl:L1407].
const ERROR_OPNAME_READ: &str = "READ";

/// `LIT-CARDFILENAME`, the `ERROR-FILE` of a failed card read
/// [app/cbl/COCRDUPC.cbl:L251-252].
const LIT_CARDFILENAME: &str = "CARDDAT ";

/// The CICS `RESP` a card read failure other than `NOTFND` reports: `IOERR`,
/// the code E-44 proposes for a database failure, which has no CICS code.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 proposes alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// `88 VALID-MONTH VALUES 1 THRU 12` [app/cbl/COCRDUPC.cbl:L92-95].
const VALID_MONTH: RangeInclusive<u16> = 1..=12;

/// `88 VALID-YEAR VALUES 1950 THRU 2099` [app/cbl/COCRDUPC.cbl:L96-99].
const VALID_YEAR: RangeInclusive<u16> = 1950..=2099;

/// The COBOL figurative constant `LOW-VALUE`.
const LOW_VALUE: u8 = 0x00;

/// One `PUT /cards/{id}` request: the `COCRDUP` map input as entered.
///
/// `card_number` is the `{id}` path segment and `account_id` the body field
/// that stands for `ACCTSID` (E-55); an absent value is passed as empty text,
/// a blank key. The four card fields are the editable map fields
/// [app/cpy-bms/COCRDUP.CPY]; the expiry day is no input, because the map
/// sends back the fetched day (P-COCRDUPC-010). Each value is moved into its
/// `X(n)` map field as typed, cut at the field's width as the map's field
/// length cuts it; the API request DTO bounds the lengths before this.
///
/// `Debug` shows the card number masked as [`CardNumber`] masks it.
#[derive(Clone, PartialEq, Eq)]
pub struct CardUpdateInput {
    /// `ACCTSIDI X(11)`.
    pub account_id: String,
    /// `CARDSIDI X(16)`.
    pub card_number: String,
    /// `CRDNAMEI X(50)`.
    pub embossed_name: String,
    /// `CRDSTCDI X(1)`.
    pub active_status: String,
    /// `EXPMONI X(2)`.
    pub expiry_month: String,
    /// `EXPYEARI X(4)`.
    pub expiry_year: String,
    /// The token of the card as the client fetched it: `CCUP-OLD-DETAILS`.
    pub version: VersionToken,
}

impl fmt::Debug for CardUpdateInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CardUpdateInput")
            .field("account_id", &self.account_id)
            .field("card_number", &CardNumber::move_from(&self.card_number))
            .field("embossed_name", &self.embossed_name)
            .field("active_status", &self.active_status)
            .field("expiry_month", &self.expiry_month)
            .field("expiry_year", &self.expiry_year)
            .field("version", &self.version)
            .finish()
    }
}

/// The keys that passed `1210-EDIT-ACCOUNT` and `1220-EDIT-CARD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardKeys {
    /// `CC-ACCT-ID-N`: the account the update record carries.
    pub account_id: AccountId,
    /// `CC-CARD-NUM`: the key the card is read, locked and rewritten by.
    pub card_number: CardNumber,
}

/// The outcome of [`validate`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardEdit {
    /// Every edit passed: `CARD-UPDATE-RECORD` as `9200-WRITE-PROCESSING`
    /// builds it, for [`apply`] to write.
    Changed(Card),
    /// The input equals the fetched card: nothing is edited or written, and
    /// the request succeeds with
    /// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`](messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED)
    /// (V-COCRDUPC-009, 200).
    NoChange,
}

/// A request that succeeded: the card as it now stands and the information
/// text the program shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardUpdated {
    /// The record rewritten, or the fetched card when nothing changed.
    pub card: Card,
    /// [`CHANGES_COMMITTED_TO_DATABASE`](messages::CHANGES_COMMITTED_TO_DATABASE)
    /// after a rewrite (P-COCRDUPC-007), or
    /// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`](messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED)
    /// when nothing changed (V-COCRDUPC-009).
    pub message: &'static str,
}

/// The outcome of one key edit: the `FLG-ACCTFILTER-*` or `FLG-CARDFILTER-*`
/// flag it sets, with the rejection its message makes.
enum KeyEdit<T> {
    /// `FLG-*FILTER-BLANK`: blank, low-values or zeros.
    Blank(DomainError),
    /// `FLG-*FILTER-NOT-OK`: not numeric.
    NotOk(DomainError),
    /// `FLG-*FILTER-ISVALID`.
    Valid(T),
}

/// `CCUP-NEW-CARDDATA` as `1100-RECEIVE-MAP` leaves it
/// [app/cbl/COCRDUPC.cbl:L307-313]: name, expiry year, month and day, status.
struct NewCardData {
    /// `CCUP-NEW-CRDNAME X(50)`.
    name: Alnum<50>,
    /// `CCUP-NEW-EXPYEAR X(4)`.
    year: Alnum<4>,
    /// `CCUP-NEW-EXPMON X(2)`.
    month: Alnum<2>,
    /// `CCUP-NEW-EXPDAY X(2)`: the fetched day the map sends back.
    day: [u8; 2],
    /// `CCUP-NEW-CRDSTCD X(1)`.
    status: Alnum<1>,
}

impl NewCardData {
    /// COCRDUPC 1100-RECEIVE-MAP — P-COCRDUPC-010
    ///
    /// The card fields of `input`, each received as [`receive`] moves it, and
    /// the expiry day of `current`: `EXPDAYI` is a protected field that
    /// `3200-SETUP-SCREEN-VARS` fills with `CCUP-OLD-EXPDAY`, so the map
    /// returns the fetched day [app/cbl/COCRDUPC.cbl:L621].
    fn from_input(input: &CardUpdateInput, current: &Card) -> Self {
        let (_, _, day) = expiry_parts(&current.card_expiraion_date);
        Self {
            name: receive(&input.embossed_name),
            year: receive(&input.expiry_year),
            month: receive(&input.expiry_month),
            day,
            status: receive(&input.active_status),
        }
    }

    /// The 59 bytes of the `CCUP-NEW-CARDDATA` group.
    fn group_bytes(&self) -> Vec<u8> {
        [
            self.name.as_bytes().as_slice(),
            self.year.as_bytes(),
            self.month.as_bytes(),
            &self.day,
            self.status.as_bytes(),
        ]
        .concat()
    }
}

/// COCRDUPC 1100-RECEIVE-MAP — P-COCRDUPC-010
///
/// The map field `text` moved into its `X(N)` receiver, or `LOW-VALUES` when
/// it holds `'*'` or `SPACES` [app/cbl/COCRDUPC.cbl:L588-636]: empty text,
/// spaces and a lone `*` are all a value not entered.
fn receive<const N: usize>(text: &str) -> Alnum<N> {
    let field = Alnum::<N>::move_from(text);
    if field.cobol_cmp(b"").is_eq() || field.cobol_cmp(b"*").is_eq() {
        Alnum::from_array([LOW_VALUE; N])
    } else {
        field
    }
}

/// The condition `= LOW-VALUES OR = SPACES OR = ZEROS` of the edits.
///
/// On the card fields the zero test is the alphanumeric comparison with
/// `ZEROS`: every byte is `0`. On the keys it reads the numeric redefine
/// (`CC-ACCT-ID-N`, `CC-CARD-NUM-N`), which agrees with it for a value of
/// digits. For a value holding a non-digit that test is unpinned (E-47, at
/// `halt`); the rules give it the outcome E-47 proposes and the reviewed
/// examples of V-COCRDUPC-003 and V-COCRDUPC-005 record: the zero test is
/// false, so the `NUMERIC` test that follows rejects the value with its own
/// text.
fn is_blank_or_zeros<const N: usize>(field: &Alnum<N>) -> bool {
    field.is_blank() || field.as_bytes().iter().all(|&byte| byte == b'0')
}

/// The value of a field of digits, as its `9(n)` redefine reads it; `None`
/// when the field holds a non-digit.
///
/// A non-digit month or year is unpinned in the COBOL (E-47, at `halt`): its
/// `VALID-MONTH` or `VALID-YEAR` test reads invalid zoned data. The edits give
/// it the outcome E-47 proposes, the range test failing, so the field's range
/// text applies.
fn digits_value<const N: usize>(field: &Alnum<N>) -> Option<u16> {
    if field.is_numeric() {
        field.as_text().parse().ok()
    } else {
        None
    }
}

/// COCRDUPC 9000-READ-DATA, 9300-CHECK-CHANGE-IN-REC — P-COCRDUPC-022,
/// P-COCRDUPC-005
///
/// The year `(1:4)`, month `(6:2)` and day `(9:2)` of a
/// `CARD-EXPIRAION-DATE X(10)`, as `9000-READ-DATA` and
/// `9300-CHECK-CHANGE-IN-REC` cut them.
fn expiry_parts(date: &Alnum<10>) -> ([u8; 4], [u8; 2], [u8; 2]) {
    let [y1, y2, y3, y4, _, m1, m2, _, d1, d2] = *date.as_bytes();
    ([y1, y2, y3, y4], [m1, m2], [d1, d2])
}

/// `INSPECT … CONVERTING LIT-LOWER TO LIT-UPPER` [app/cbl/COCRDUPC.cbl:L1356-1358]:
/// `a` to `z` become `A` to `Z`, every other byte is kept.
fn upper_cased<const N: usize>(field: &Alnum<N>) -> [u8; N] {
    field.as_bytes().map(|byte| byte.to_ascii_uppercase())
}

/// COCRDUPC 9300-CHECK-CHANGE-IN-REC — P-COCRDUPC-005, P-COCRDUPC-022
///
/// The [`VersionToken`] of `card` under `key`: the keyed digest of exactly
/// the fields `9300-CHECK-CHANGE-IN-REC` compares, in its order
/// [app/cbl/COCRDUPC.cbl:L1503-1508]: `CARD-CVV-CD` as its three digits, the
/// embossed name upper-cased as `INSPECT … CONVERTING LIT-LOWER TO LIT-UPPER`
/// upper-cases it (only `a` to `z`), the expiry year `(1:4)`, month `(6:2)`
/// and day `(9:2)`, and `CARD-ACTIVE-STATUS`. `9000-READ-DATA` stores the
/// fetched name upper-cased the same way, so under one key two cards get
/// equal tokens exactly when the change check finds no change between them.
///
/// Every compared field but the verification code is in the card response,
/// so only the key keeps the token from revealing that code: without it, no
/// client can test the 1,000 candidates against the token it was given.
///
/// # Errors
///
/// [`ErrorKind::Unexpected`] with
/// [`UNEXPECTED_ERROR`](messages::UNEXPECTED_ERROR) at `ACCTSID` when the
/// digest fails; as [`version_token`] documents, no card's fields reach the
/// input size that makes it fail.
pub fn record_version(key: &VersionKey, card: &Card) -> Result<VersionToken, DomainError> {
    let cvv = card.card_cvv_cd.to_string();
    let name = upper_cased(&card.card_embossed_name);
    let (year, month, day) = expiry_parts(&card.card_expiraion_date);
    version_token(
        key,
        &[
            cvv.as_bytes(),
            &name,
            &year,
            &month,
            &day,
            card.card_active_status.as_bytes(),
        ],
    )
    .map_err(|_| DomainError::unexpected(messages::UNEXPECTED_ERROR).with_field(FIELD_ACCTSID))
}

/// COCRDUPC 1210-EDIT-ACCOUNT — P-COCRDUPC-011, V-COCRDUPC-002,
/// V-COCRDUPC-003
///
/// `CC-ACCT-ID` blank, low-values or zero is not provided (`Empty`); one that
/// is not 11 digits is rejected (`Invalid`); both at `ACCTSID`. No file is
/// read.
fn edit_account(text: &str) -> KeyEdit<AccountId> {
    let field = receive::<11>(text);
    if is_blank_or_zeros(&field) {
        return KeyEdit::Blank(
            DomainError::empty(messages::ACCOUNT_NUMBER_NOT_PROVIDED).with_field(FIELD_ACCTSID),
        );
    }
    let value = if field.is_numeric() {
        field.as_text().parse::<AccountId>().ok()
    } else {
        None
    };
    value.map_or_else(
        || {
            KeyEdit::NotOk(
                DomainError::invalid(messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT)
                    .with_field(FIELD_ACCTSID),
            )
        },
        KeyEdit::Valid,
    )
}

/// COCRDUPC 1220-EDIT-CARD — P-COCRDUPC-012, V-COCRDUPC-004, V-COCRDUPC-005
///
/// `CC-CARD-NUM` blank, low-values or zero is not provided (`Empty`); one that
/// is not 16 digits is rejected (`Invalid`); both at `CARDSID`. No file is
/// read.
fn edit_card(text: &str) -> KeyEdit<CardNumber> {
    let field = receive::<16>(text);
    if is_blank_or_zeros(&field) {
        KeyEdit::Blank(
            DomainError::empty(messages::CARD_NUMBER_NOT_PROVIDED).with_field(FIELD_CARDSID),
        )
    } else if field.is_numeric() {
        KeyEdit::Valid(CardNumber::from_alnum(field))
    } else {
        KeyEdit::NotOk(
            DomainError::invalid(messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT)
                .with_field(FIELD_CARDSID),
        )
    }
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 1210-EDIT-ACCOUNT, 1220-EDIT-CARD —
/// P-COCRDUPC-011, P-COCRDUPC-012, V-COCRDUPC-002 to V-COCRDUPC-006
///
/// The key edits of the first turn [app/cbl/COCRDUPC.cbl:L645-663]: the
/// account, then the card number. The account's text stands when both fail;
/// when both are blank (or zero), `SET NO-SEARCH-CRITERIA-RECEIVED` replaces
/// either text with [`NO_INPUT_RECEIVED`](messages::NO_INPUT_RECEIVED) at
/// `ACCTSID`. The card is then read by its number alone; the account is not
/// compared with the card's (E-55).
///
/// # Errors
///
/// The first failing key edit, as a [`DomainError`] of kind
/// [`ErrorKind::Empty`] or [`ErrorKind::Invalid`] at `ACCTSID` or `CARDSID`.
pub fn edit_keys(account_id: &str, card_number: &str) -> Result<CardKeys, DomainError> {
    match (edit_account(account_id), edit_card(card_number)) {
        (KeyEdit::Valid(account_id), KeyEdit::Valid(card_number)) => Ok(CardKeys {
            account_id,
            card_number,
        }),
        (KeyEdit::Blank(_), KeyEdit::Blank(_)) => {
            Err(DomainError::empty(messages::NO_INPUT_RECEIVED).with_field(FIELD_ACCTSID))
        }
        (KeyEdit::Blank(first) | KeyEdit::NotOk(first), _)
        | (KeyEdit::Valid(_), KeyEdit::Blank(first) | KeyEdit::NotOk(first)) => Err(first),
    }
}

/// COCRDUPC 9000-READ-DATA, 9100-GETCARD-BYACCTCARD — P-COCRDUPC-022,
/// P-COCRDUPC-023, V-COCRDUPC-007, V-COCRDUPC-008
///
/// The fetch [app/cbl/COCRDUPC.cbl:L1343-1413]: CICS `READ` of `CARDDAT` by
/// card number alone, without a lock. The card it returns is the
/// `CCUP-OLD-DETAILS` source that [`validate`] compares the input with.
///
/// # Errors
///
/// - [`ErrorKind::NotFound`] with
///   [`DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION`](messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION)
///   at `ACCTSID` when no card has the number (`NOTFND`).
/// - [`ErrorKind::Unexpected`] with the
///   [`file_error_message`](messages::file_error_message) of the `READ` of
///   `CARDDAT` at `ACCTSID` on any other failure, carrying `RESP` 17 and
///   `RESP2` 0 (E-44).
pub async fn read_data<S: CardReader>(
    store: &mut S,
    card_number: CardNumber,
) -> Result<Card, DomainError> {
    store
        .read_card(card_number)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION)
                    .with_field(FIELD_ACCTSID)
            }
            _ => DomainError::unexpected(messages::file_error_message(
                ERROR_OPNAME_READ,
                LIT_CARDFILENAME,
                RESP_IOERR,
                RESP2_IOERR,
            ))
            .with_field(FIELD_ACCTSID),
        })
}

/// COCRDUPC 1230-EDIT-NAME — P-COCRDUPC-013, V-COCRDUPC-010, V-COCRDUPC-011
///
/// A name of low-values, spaces or zeros is not provided (`Empty`). Otherwise
/// `INSPECT … CONVERTING` turns the 52 letters `A`-`Z` and `a`-`z` into spaces
/// and the trimmed remainder must be empty, so only letters and spaces pass
/// (`Invalid` otherwise). The name is not upper-cased.
fn edit_name(name: &Alnum<50>) -> Result<(), DomainError> {
    if is_blank_or_zeros(name) {
        return Err(DomainError::empty(messages::CARD_NAME_NOT_PROVIDED).with_field(FIELD_CRDNAME));
    }
    if name
        .as_bytes()
        .iter()
        .all(|&byte| byte.is_ascii_alphabetic() || byte == b' ')
    {
        Ok(())
    } else {
        Err(
            DomainError::invalid(messages::CARD_NAME_CAN_ONLY_CONTAIN_ALPHABETS_AND_SPACES)
                .with_field(FIELD_CRDNAME),
        )
    }
}

/// COCRDUPC 1240-EDIT-CARDSTATUS — P-COCRDUPC-014, V-COCRDUPC-012,
/// V-COCRDUPC-013
///
/// A status of low-values, a space or `0` is `Empty`; any value but an
/// upper-case `Y` or `N` (`88 FLG-YES-NO-VALID`) is `Invalid`; both with the
/// same text.
fn edit_status(status: Alnum<1>) -> Result<(), DomainError> {
    let rejected = |kind| {
        DomainError::new(kind, messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N)
            .with_field(FIELD_CRDSTCD)
    };
    if is_blank_or_zeros(&status) {
        Err(rejected(ErrorKind::Empty))
    } else if matches!(status.as_bytes(), b"Y" | b"N") {
        Ok(())
    } else {
        Err(rejected(ErrorKind::Invalid))
    }
}

/// COCRDUPC 1250-EDIT-EXPIRY-MON — P-COCRDUPC-015, V-COCRDUPC-014,
/// V-COCRDUPC-015
///
/// A month of low-values, spaces or `00` is `Empty`; one outside
/// `VALID-MONTH` (01 to 12) is `Invalid`; both with the same text.
fn edit_expiry_month(month: Alnum<2>) -> Result<(), DomainError> {
    let rejected = |kind| {
        DomainError::new(kind, messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12)
            .with_field(FIELD_EXPMON)
    };
    if is_blank_or_zeros(&month) {
        Err(rejected(ErrorKind::Empty))
    } else if digits_value(&month).is_some_and(|value| VALID_MONTH.contains(&value)) {
        Ok(())
    } else {
        Err(rejected(ErrorKind::Invalid))
    }
}

/// COCRDUPC 1260-EDIT-EXPIRY-YEAR — P-COCRDUPC-016, V-COCRDUPC-016,
/// V-COCRDUPC-017
///
/// A year of low-values, spaces or `0000` is `Empty`; one outside
/// `VALID-YEAR` (1950 to 2099) is `Invalid`; both with the same text.
fn edit_expiry_year(year: Alnum<4>) -> Result<(), DomainError> {
    let rejected =
        |kind| DomainError::new(kind, messages::INVALID_CARD_EXPIRY_YEAR).with_field(FIELD_EXPYEAR);
    if is_blank_or_zeros(&year) {
        Err(rejected(ErrorKind::Empty))
    } else if digits_value(&year).is_some_and(|value| VALID_YEAR.contains(&value)) {
        Ok(())
    } else {
        Err(rejected(ErrorKind::Invalid))
    }
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS — P-COCRDUPC-002, V-COCRDUPC-009
///
/// `FUNCTION UPPER-CASE(CCUP-NEW-CARDDATA) EQUAL
/// FUNCTION UPPER-CASE(CCUP-OLD-CARDDATA)` [app/cbl/COCRDUPC.cbl:L680-683]:
/// the received name, year, month, day and status against the fetched ones,
/// whose name `9000-READ-DATA` stored upper-cased. A field not entered is
/// low-values and so differs from any fetched value. `UPPER-CASE` is applied
/// as the ASCII conversion of `a` to `z`: the parity corpus is ASCII (E-05)
/// and every name an update stores passed [`edit_name`].
fn no_changes_detected(new: &NewCardData, current: &Card) -> bool {
    let (year, month, day) = expiry_parts(&current.card_expiraion_date);
    let mut fetched = [
        upper_cased(&current.card_embossed_name).as_slice(),
        &year,
        &month,
        &day,
        current.card_active_status.as_bytes(),
    ]
    .concat();
    let mut received = new.group_bytes();
    fetched.make_ascii_uppercase();
    received.make_ascii_uppercase();
    received == fetched
}

/// COCRDUPC 9200-WRITE-PROCESSING — P-COCRDUPC-006, P-COCRDUPC-028
///
/// `CARD-UPDATE-RECORD` [app/cbl/COCRDUPC.cbl:L1461-1475]: the card number
/// and account of the keys, the name and status as entered, and the expiry
/// date `STRING`ed as year, `-`, month, `-` and the fetched day.
///
/// The verification code is the stored one. The COBOL moves
/// `CCUP-NEW-CVV-CD`, which holds the spaces of the `INITIALIZE` in
/// `1100-RECEIVE-MAP`, through its `9(03)` redefine, and what that `MOVE`
/// stores is unpinned (E-75, at `halt`); keeping the stored code is the
/// replacement E-75 decides.
fn update_record(keys: CardKeys, new: &NewCardData, current: &Card) -> Card {
    let expiry = [
        new.year.as_bytes().as_slice(),
        b"-",
        new.month.as_bytes(),
        b"-",
        &new.day,
    ]
    .concat();
    Card {
        card_num: keys.card_number,
        card_acct_id: keys.account_id,
        card_cvv_cd: current.card_cvv_cd,
        card_embossed_name: new.name,
        card_expiraion_date: Alnum::move_from_bytes(&expiry),
        card_active_status: new.status,
    }
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 1230-EDIT-NAME to 1260-EDIT-EXPIRY-YEAR —
/// P-COCRDUPC-002, P-COCRDUPC-013 to P-COCRDUPC-016, V-COCRDUPC-002 to
/// V-COCRDUPC-006, V-COCRDUPC-009 to V-COCRDUPC-017
///
/// The ENTER turn against `current`, the card [`read_data`] fetched: the keys
/// of `input` are edited as [`edit_keys`] edits them; then the received card
/// fields are compared with `current`, and when they differ the name, status,
/// expiry month and expiry year edits all run
/// [app/cbl/COCRDUPC.cbl:L696-714], the first failure supplying the error.
///
/// # Errors
///
/// The first failing key edit, as [`edit_keys`] returns it; otherwise the
/// first failing card-field edit in the order name, status, month, year:
/// [`ErrorKind::Empty`] for a field not provided, [`ErrorKind::Invalid`] for
/// one its edit rejects, at `CRDNAME`, `CRDSTCD`, `EXPMON` or `EXPYEAR`.
pub fn validate(current: &Card, input: &CardUpdateInput) -> Result<CardEdit, DomainError> {
    let keys = edit_keys(&input.account_id, &input.card_number)?;
    edit_card_fields(keys, current, input)
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 1230-EDIT-NAME to 1260-EDIT-EXPIRY-YEAR —
/// P-COCRDUPC-002, P-COCRDUPC-013 to P-COCRDUPC-016, V-COCRDUPC-009 to
/// V-COCRDUPC-017
///
/// The ENTER turn after the key edits, against `current`, the card
/// [`read_data`] fetched, with `keys`, the keys of `input` [`edit_keys`]
/// passed: the received card fields are compared with `current`, and when
/// they differ the name, status, expiry month and expiry year edits all run
/// [app/cbl/COCRDUPC.cbl:L696-714], the first failure supplying the error;
/// when all pass, the update record carries `keys`.
///
/// # Errors
///
/// The first failing card-field edit in the order name, status, month, year:
/// [`ErrorKind::Empty`] for a field not provided, [`ErrorKind::Invalid`] for
/// one its edit rejects, at `CRDNAME`, `CRDSTCD`, `EXPMON` or `EXPYEAR`.
fn edit_card_fields(
    keys: CardKeys,
    current: &Card,
    input: &CardUpdateInput,
) -> Result<CardEdit, DomainError> {
    let new = NewCardData::from_input(input, current);
    if no_changes_detected(&new, current) {
        return Ok(CardEdit::NoChange);
    }
    [
        edit_name(&new.name),
        edit_status(new.status),
        edit_expiry_month(new.month),
        edit_expiry_year(new.year),
    ]
    .into_iter()
    .collect::<Result<(), DomainError>>()?;
    Ok(CardEdit::Changed(update_record(keys, &new, current)))
}

/// COCRDUPC 9300-CHECK-CHANGE-IN-REC — P-COCRDUPC-005, V-COCRDUPC-019
///
/// The locked record against the fetched values the token stands for
/// [app/cbl/COCRDUPC.cbl:L1498-1521], the locked record's token computed
/// under `key`, the key the fetched token was issued under.
///
/// The COBOL then refreshes `CCUP-OLD-DETAILS` from the locked record
/// [app/cbl/COCRDUPC.cbl:L1512-1517] and redisplays it. The error carries no
/// card: the API error body is exactly `{code, message, fields}`, so the
/// client reads the current card, with its new token, through
/// `GET /cards/{id}`.
fn check_change_in_rec(
    key: &VersionKey,
    locked: &Card,
    expected: &VersionToken,
) -> Result<(), DomainError> {
    if record_version(key, locked)? == *expected {
        Ok(())
    } else {
        Err(
            DomainError::conflict(messages::RECORD_CHANGED_BY_SOME_ONE_ELSE)
                .with_field(FIELD_CRDNAME),
        )
    }
}

/// COCRDUPC 9200-WRITE-PROCESSING, 9300-CHECK-CHANGE-IN-REC, 3250-SETUP-INFOMSG
/// — P-COCRDUPC-004 to P-COCRDUPC-007, P-COCRDUPC-028, V-COCRDUPC-018 to
/// V-COCRDUPC-020
///
/// The PF5 write [app/cbl/COCRDUPC.cbl:L1420-1496]: `READ … UPDATE` of
/// `card_number`, the change check of the locked record against `expected`
/// with its token computed under `key`, then one `REWRITE` of `updated`, the
/// record [`validate`] built. The record
/// is written under `card_number`, the key it was locked by, and with the
/// locked record's verification code (E-75). With a single write,
/// `RECOVERY(NONE)` of `CARDDAT` [app/csd/CARDDEMO.CSD:L25-33] changes
/// nothing: a failed rewrite leaves the card as it was.
///
/// # Errors
///
/// - [`ErrorKind::Unexpected`] with
///   [`COULD_NOT_LOCK_RECORD_FOR_UPDATE`](messages::COULD_NOT_LOCK_RECORD_FOR_UPDATE)
///   at `ACCTSID` when the locked read fails for any reason; nothing is
///   written.
/// - [`ErrorKind::Unexpected`] with
///   [`UNEXPECTED_ERROR`](messages::UNEXPECTED_ERROR) at `ACCTSID` when the
///   locked record's token cannot be computed ([`record_version`]); nothing is
///   written.
/// - [`ErrorKind::Conflict`] with
///   [`RECORD_CHANGED_BY_SOME_ONE_ELSE`](messages::RECORD_CHANGED_BY_SOME_ONE_ELSE)
///   at `CRDNAME` when the locked record's token differs from `expected`;
///   nothing is written.
/// - [`ErrorKind::Unexpected`] with
///   [`UPDATE_OF_RECORD_FAILED`](messages::UPDATE_OF_RECORD_FAILED) at
///   `ACCTSID` when the rewrite fails.
pub async fn apply<S: CardReader + CardWriter>(
    store: &mut S,
    key: &VersionKey,
    card_number: CardNumber,
    expected: &VersionToken,
    updated: Card,
) -> Result<CardUpdated, DomainError> {
    let locked = store.read_card_for_update(card_number).await.map_err(|_| {
        DomainError::unexpected(messages::COULD_NOT_LOCK_RECORD_FOR_UPDATE)
            .with_field(FIELD_ACCTSID)
    })?;
    check_change_in_rec(key, &locked, expected)?;
    let record = Card {
        card_num: card_number,
        card_cvv_cd: locked.card_cvv_cd,
        ..updated
    };
    store.rewrite_card(record.clone()).await.map_err(|_| {
        DomainError::unexpected(messages::UPDATE_OF_RECORD_FAILED).with_field(FIELD_ACCTSID)
    })?;
    Ok(CardUpdated {
        card: record,
        message: messages::CHANGES_COMMITTED_TO_DATABASE,
    })
}

/// COCRDUPC 1200-EDIT-MAP-INPUTS, 2000-DECIDE-ACTION, 9000-READ-DATA,
/// 9200-WRITE-PROCESSING — P-COCRDUPC-002 to P-COCRDUPC-007,
/// P-COCRDUPC-009 to P-COCRDUPC-016, P-COCRDUPC-022, P-COCRDUPC-023,
/// V-COCRDUPC-002 to V-COCRDUPC-020
///
/// The whole `PUT /cards/{id}`: [`edit_keys`], [`read_data`], the card-field
/// edits of [`validate`] for the keys already edited and, when the input
/// changes the card, [`apply`] with `key`, the key the token of `input` was
/// issued under. When nothing changed the
/// request succeeds with the fetched card and
/// [`NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED`](messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED)
/// and nothing is locked or written. The caller owns the database
/// transaction and commits it.
///
/// # Errors
///
/// The error of the first step that fails, as that step documents it, in the
/// order the validation audit ranks them: key edits, the fetch, the card-field
/// edits, the lock, the change check, the rewrite.
pub async fn update<S: CardReader + CardWriter>(
    store: &mut S,
    key: &VersionKey,
    input: &CardUpdateInput,
) -> Result<CardUpdated, DomainError> {
    let keys = edit_keys(&input.account_id, &input.card_number)?;
    let current = read_data(store, keys.card_number).await?;
    match edit_card_fields(keys, &current, input)? {
        CardEdit::NoChange => Ok(CardUpdated {
            card: current,
            message: messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
        }),
        CardEdit::Changed(record) => {
            apply(store, key, keys.card_number, &input.version, record).await
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;

    use super::{
        CardEdit, CardKeys, CardUpdateInput, CardUpdated, apply, edit_keys, read_data,
        record_version, update, validate,
    };
    use crate::entities::Card;
    use crate::error::{DomainError, ErrorKind};
    use crate::messages;
    use crate::numeric::{AccountId, Alnum, CardNumber, Cvv};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on, version_key};
    use crate::rules::{VersionKey, VersionToken};

    /// The first fixture card, of account 00000000050 (P-CVACT02Y-009)
    /// [app/data/ASCII/carddata.txt:L1].
    const CARD: &str = "0500024453765740";

    /// The account of [`CARD`].
    const ACCOUNT: &str = "00000000050";

    /// A card number no fixture card has (P-CVACT02Y-010).
    const MISSING_CARD: &str = "9999999999999999";

    fn card_number() -> CardNumber {
        CardNumber::move_from(CARD)
    }

    fn account(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn cvv(value: u16) -> Cvv {
        Cvv::new(value).expect("a three-digit code")
    }

    /// [`CARD`] as the fixture holds it.
    fn fixture_card() -> Card {
        Card {
            card_num: card_number(),
            card_acct_id: account(50),
            card_cvv_cd: cvv(747),
            card_embossed_name: Alnum::move_from("Aniya Von"),
            card_expiraion_date: Alnum::move_from("2023-03-09"),
            card_active_status: Alnum::move_from("Y"),
        }
    }

    /// A store holding [`fixture_card`] alone.
    fn store() -> FakeStore {
        FakeStore::default().with_card(fixture_card())
    }

    /// The token of `card` under the tests' server key, as `GET /cards/{id}`
    /// issues it.
    fn version_of(card: &Card) -> VersionToken {
        record_version(&version_key(), card).expect("the token of a card")
    }

    /// The input that repeats [`fixture_card`] as `GET /cards/{id}` shows it.
    fn unchanged_input() -> CardUpdateInput {
        CardUpdateInput {
            account_id: ACCOUNT.to_owned(),
            card_number: CARD.to_owned(),
            embossed_name: "Aniya Von".to_owned(),
            active_status: "Y".to_owned(),
            expiry_month: "03".to_owned(),
            expiry_year: "2023".to_owned(),
            version: version_of(&fixture_card()),
        }
    }

    /// [`unchanged_input`] with the expiry month moved to April, so the card
    /// fields differ from the fetched ones and every edit runs.
    fn changed_input() -> CardUpdateInput {
        CardUpdateInput {
            expiry_month: "04".to_owned(),
            ..unchanged_input()
        }
    }

    /// [`changed_input`] after `edit`.
    fn changed_with(edit: impl FnOnce(&mut CardUpdateInput)) -> CardUpdateInput {
        let mut input = changed_input();
        edit(&mut input);
        input
    }

    /// The update record [`validate`] builds for `input` against `current`.
    fn update_record(current: &Card, input: &CardUpdateInput) -> Card {
        match validate(current, input) {
            Ok(CardEdit::Changed(record)) => record,
            other => panic!("the input should validate as a change, got {other:?}"),
        }
    }

    /// Asserts that `result` is the rejection `kind`, `message` at `field`.
    fn assert_rejected<T: Debug>(
        result: Result<T, DomainError>,
        (kind, message, field): (ErrorKind, &str, &str),
        case: &str,
    ) {
        match result {
            Err(error) => {
                assert_eq!(error.kind, kind, "{case}");
                assert_eq!(error.code, kind.code(), "{case}");
                assert_eq!(error.message, message, "{case}");
                assert_eq!(error.first_field(), Some(field), "{case}");
                assert_eq!(error.fields.len(), 1, "{case}");
            }
            Ok(value) => panic!("{case}: expected a rejection, got {value:?}"),
        }
    }

    #[test]
    fn record_version_digests_the_compared_fields_with_the_name_upper_cased() {
        // P-COCRDUPC-005, P-COCRDUPC-022
        let card = fixture_card();
        let token = version_of(&card);
        let recased = Card {
            card_embossed_name: Alnum::move_from("ANIYA VON"),
            ..card.clone()
        };
        assert_eq!(version_of(&recased), token);
        let other_account = Card {
            card_acct_id: account(1),
            ..card.clone()
        };
        assert_eq!(
            version_of(&other_account),
            token,
            "the account is not compared"
        );

        let changed = [
            Card {
                card_cvv_cd: cvv(748),
                ..card.clone()
            },
            Card {
                card_embossed_name: Alnum::move_from("Aniya Vom"),
                ..card.clone()
            },
            Card {
                card_expiraion_date: Alnum::move_from("2024-03-09"),
                ..card.clone()
            },
            Card {
                card_expiraion_date: Alnum::move_from("2023-04-09"),
                ..card.clone()
            },
            Card {
                card_expiraion_date: Alnum::move_from("2023-03-10"),
                ..card.clone()
            },
            Card {
                card_active_status: Alnum::move_from("N"),
                ..card.clone()
            },
        ];
        for other in changed {
            assert_ne!(version_of(&other), token, "{other:?}");
        }
    }

    #[test]
    fn the_token_does_not_disclose_the_verification_code() {
        // P-COCRDUPC-005: the card response shows every compared field but the
        // CVV, so a client can rebuild all 1,000 candidate records. Without
        // the server key none of their tokens matches the one it was given.
        let card = fixture_card();
        let server_key = version_key();
        let shown = record_version(&server_key, &card).expect("the server's token");
        let matching_codes = |key: &VersionKey| -> Vec<u16> {
            (0..=999)
                .filter(|&code| {
                    let candidate = Card {
                        card_cvv_cd: cvv(code),
                        ..card.clone()
                    };
                    record_version(key, &candidate).expect("a candidate's token") == shown
                })
                .collect()
        };
        let guessed_key =
            VersionKey::new(b"a client's guess at the server secret").expect("a non-empty secret");
        assert_eq!(matching_codes(&guessed_key), Vec::<u16>::new());
        assert_eq!(matching_codes(&server_key), [747]);
    }

    #[test]
    fn a_token_issued_under_another_key_is_a_changed_record() {
        // V-COCRDUPC-019: the locked card's token is computed under the
        // server key, so a token computed under any other key never matches.
        let other_key = VersionKey::new(b"another server secret").expect("a non-empty secret");
        let foreign = record_version(&other_key, &fixture_card()).expect("a token");
        let mut store = store();
        let record = update_record(&fixture_card(), &changed_input());
        assert_rejected(
            block_on(apply(
                &mut store,
                &version_key(),
                card_number(),
                &foreign,
                record,
            )),
            (
                ErrorKind::Conflict,
                messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                "CRDNAME",
            ),
            "a token of another key",
        );
        assert_eq!(store.cards.get(&card_number()), Some(&fixture_card()));
        assert_eq!(store.calls, ["read_card_for_update"]);
    }

    #[test]
    fn key_edits_keep_the_first_text_and_replace_two_blank_keys() {
        let not_provided = (
            ErrorKind::Empty,
            messages::ACCOUNT_NUMBER_NOT_PROVIDED,
            "ACCTSID",
        );
        let account_not_numeric = (
            ErrorKind::Invalid,
            messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT,
            "ACCTSID",
        );
        let card_not_provided = (
            ErrorKind::Empty,
            messages::CARD_NUMBER_NOT_PROVIDED,
            "CARDSID",
        );
        let card_not_numeric = (
            ErrorKind::Invalid,
            messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
            "CARDSID",
        );
        let no_input = (ErrorKind::Empty, messages::NO_INPUT_RECEIVED, "ACCTSID");
        let cases = [
            // V-COCRDUPC-002: blank, '*' (1100-RECEIVE-MAP) and zeros.
            ("", CARD, not_provided),
            ("*", CARD, not_provided),
            ("00000000000", CARD, not_provided),
            // V-COCRDUPC-003, with E-47's outcome for non-digits.
            ("0000000005A", CARD, account_not_numeric),
            ("50", CARD, account_not_numeric),
            // V-COCRDUPC-004.
            (ACCOUNT, "0000000000000000", card_not_provided),
            (ACCOUNT, "   ", card_not_provided),
            // V-COCRDUPC-005.
            (ACCOUNT, "050002445376574X", card_not_numeric),
            // V-COCRDUPC-006: the SET replaces either prompt.
            ("", " ", no_input),
            ("00000000000", "0000000000000000", no_input),
            // The account edit runs first, so its text stands.
            ("0000000005A", "", account_not_numeric),
            ("", "050002445376574X", not_provided),
        ];
        for (account_id, card, expected) in cases {
            assert_rejected(
                edit_keys(account_id, card),
                expected,
                &format!("{account_id:?} / {card:?}"),
            );
        }
        assert_eq!(
            edit_keys(ACCOUNT, CARD),
            Ok(CardKeys {
                account_id: account(50),
                card_number: card_number(),
            })
        );
    }

    #[test]
    fn read_data_reads_by_card_number_and_maps_its_failures() {
        let mut store = store();
        assert_eq!(
            block_on(read_data(&mut store, card_number())),
            Ok(fixture_card())
        );

        // V-COCRDUPC-007.
        assert_rejected(
            block_on(read_data(&mut store, CardNumber::move_from(MISSING_CARD))),
            (
                ErrorKind::NotFound,
                messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION,
                "ACCTSID",
            ),
            "missing card",
        );

        // V-COCRDUPC-008, with the RESP and RESP2 E-44 proposes.
        store.fail_next(
            "read_card",
            PortError::Unavailable("connection reset".to_owned()),
        );
        let file_error = messages::file_error_message("READ", "CARDDAT ", 17, 0);
        assert_rejected(
            block_on(read_data(&mut store, card_number())),
            (ErrorKind::Unexpected, &file_error, "ACCTSID"),
            "read failure",
        );
        assert_eq!(store.calls, ["read_card"; 3]);
    }

    #[test]
    fn validate_finds_no_change_after_upper_casing_both_sides() {
        // V-COCRDUPC-009: no edit runs, so a lower-case status passes too.
        let card = fixture_card();
        let same = [
            unchanged_input(),
            CardUpdateInput {
                embossed_name: "ANIYA VON".to_owned(),
                ..unchanged_input()
            },
            CardUpdateInput {
                embossed_name: "aniya von".to_owned(),
                ..unchanged_input()
            },
            CardUpdateInput {
                active_status: "y".to_owned(),
                ..unchanged_input()
            },
        ];
        for input in same {
            assert_eq!(validate(&card, &input), Ok(CardEdit::NoChange), "{input:?}");
        }
    }

    #[test]
    fn each_card_field_edit_rejects_with_its_text_kind_and_field() {
        let name_blank = (
            ErrorKind::Empty,
            messages::CARD_NAME_NOT_PROVIDED,
            "CRDNAME",
        );
        let name_invalid = (
            ErrorKind::Invalid,
            messages::CARD_NAME_CAN_ONLY_CONTAIN_ALPHABETS_AND_SPACES,
            "CRDNAME",
        );
        let status = |kind| (kind, messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N, "CRDSTCD");
        let month = |kind| {
            (
                kind,
                messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12,
                "EXPMON",
            )
        };
        let year = |kind| (kind, messages::INVALID_CARD_EXPIRY_YEAR, "EXPYEAR");
        let (empty, invalid) = (ErrorKind::Empty, ErrorKind::Invalid);
        let cases = [
            // V-COCRDUPC-010.
            (
                changed_with(|input| input.embossed_name.clear()),
                name_blank,
            ),
            (
                changed_with(|input| input.embossed_name = "*".to_owned()),
                name_blank,
            ),
            (
                changed_with(|input| input.embossed_name = "0".repeat(50)),
                name_blank,
            ),
            // V-COCRDUPC-011.
            (
                changed_with(|input| input.embossed_name = "Aniya V0n".to_owned()),
                name_invalid,
            ),
            (
                changed_with(|input| input.embossed_name = "Aniya-Von".to_owned()),
                name_invalid,
            ),
            // V-COCRDUPC-012.
            (
                changed_with(|input| input.active_status.clear()),
                status(empty),
            ),
            (
                changed_with(|input| input.active_status = "0".to_owned()),
                status(empty),
            ),
            // V-COCRDUPC-013.
            (
                changed_with(|input| input.active_status = "y".to_owned()),
                status(invalid),
            ),
            (
                changed_with(|input| input.active_status = "X".to_owned()),
                status(invalid),
            ),
            // V-COCRDUPC-014.
            (
                changed_with(|input| input.expiry_month.clear()),
                month(empty),
            ),
            (
                changed_with(|input| input.expiry_month = "00".to_owned()),
                month(empty),
            ),
            // V-COCRDUPC-015, with E-47's outcome for non-digits.
            (
                changed_with(|input| input.expiry_month = "13".to_owned()),
                month(invalid),
            ),
            (
                changed_with(|input| input.expiry_month = "1A".to_owned()),
                month(invalid),
            ),
            (
                changed_with(|input| input.expiry_month = "5".to_owned()),
                month(invalid),
            ),
            // V-COCRDUPC-016.
            (changed_with(|input| input.expiry_year.clear()), year(empty)),
            (
                changed_with(|input| input.expiry_year = "0000".to_owned()),
                year(empty),
            ),
            // V-COCRDUPC-017, with E-47's outcome for non-digits.
            (
                changed_with(|input| input.expiry_year = "1949".to_owned()),
                year(invalid),
            ),
            (
                changed_with(|input| input.expiry_year = "2100".to_owned()),
                year(invalid),
            ),
            (
                changed_with(|input| input.expiry_year = "20X5".to_owned()),
                year(invalid),
            ),
        ];
        let card = fixture_card();
        for (input, expected) in cases {
            let case = format!("{input:?}");
            assert_rejected(validate(&card, &input), expected, &case);
        }
    }

    #[test]
    fn every_card_field_edit_runs_and_the_first_failure_wins() {
        // P-COCRDUPC-013 to P-COCRDUPC-016: name, status, month, year.
        let card = fixture_card();
        let cases = [
            (
                changed_with(|input| {
                    input.embossed_name = "Aniya V0n".to_owned();
                    input.active_status = "X".to_owned();
                }),
                (
                    ErrorKind::Invalid,
                    messages::CARD_NAME_CAN_ONLY_CONTAIN_ALPHABETS_AND_SPACES,
                    "CRDNAME",
                ),
            ),
            (
                changed_with(|input| {
                    input.active_status.clear();
                    input.expiry_year = "1949".to_owned();
                }),
                (
                    ErrorKind::Empty,
                    messages::CARD_ACTIVE_STATUS_MUST_BE_Y_OR_N,
                    "CRDSTCD",
                ),
            ),
            (
                changed_with(|input| {
                    input.expiry_month = "13".to_owned();
                    input.expiry_year.clear();
                }),
                (
                    ErrorKind::Invalid,
                    messages::CARD_EXPIRY_MONTH_MUST_BE_BETWEEN_1_AND_12,
                    "EXPMON",
                ),
            ),
        ];
        for (input, expected) in cases {
            let case = format!("{input:?}");
            assert_rejected(validate(&card, &input), expected, &case);
        }
    }

    #[test]
    fn validate_accepts_the_range_bounds() {
        let card = fixture_card();
        for (month, year) in [("01", "1950"), ("12", "2099")] {
            let input = changed_with(|input| {
                input.expiry_month = month.to_owned();
                input.expiry_year = year.to_owned();
            });
            assert!(
                matches!(validate(&card, &input), Ok(CardEdit::Changed(_))),
                "{month}/{year}"
            );
        }
    }

    #[test]
    fn validate_builds_the_update_record_from_the_input_and_the_fetched_card() {
        // P-COCRDUPC-006: the account of the input (E-55), the name as entered
        // (P-COCRDUPC-013), the fetched day (P-COCRDUPC-010) and the stored
        // verification code (E-75).
        let input = CardUpdateInput {
            account_id: "00000000001".to_owned(),
            embossed_name: "aniya j von".to_owned(),
            active_status: "N".to_owned(),
            expiry_month: "12".to_owned(),
            expiry_year: "2027".to_owned(),
            ..unchanged_input()
        };
        let expected = Card {
            card_num: card_number(),
            card_acct_id: account(1),
            card_cvv_cd: cvv(747),
            card_embossed_name: Alnum::move_from("aniya j von"),
            card_expiraion_date: Alnum::move_from("2027-12-09"),
            card_active_status: Alnum::move_from("N"),
        };
        assert_eq!(
            validate(&fixture_card(), &input),
            Ok(CardEdit::Changed(expected))
        );
    }

    #[test]
    fn apply_rewrites_the_locked_card_and_reports_the_commit() {
        // P-COCRDUPC-006, P-COCRDUPC-007.
        let mut store = store();
        let record = update_record(&fixture_card(), &changed_input());
        let token = version_of(&fixture_card());
        assert_eq!(
            block_on(apply(
                &mut store,
                &version_key(),
                card_number(),
                &token,
                record.clone()
            )),
            Ok(CardUpdated {
                card: record.clone(),
                message: messages::CHANGES_COMMITTED_TO_DATABASE,
            })
        );
        assert_eq!(store.cards.get(&card_number()), Some(&record));
        assert_eq!(store.calls, ["read_card_for_update", "rewrite_card"]);
    }

    #[test]
    fn apply_keeps_the_stored_verification_code_and_the_locked_key() {
        // E-75: the CVV of the locked record is written, whatever the record
        // passed holds; the record goes to the key that was locked.
        let mut store = store();
        let record = Card {
            card_num: CardNumber::move_from("4111111111111111"),
            card_cvv_cd: cvv(1),
            ..update_record(&fixture_card(), &changed_input())
        };
        let token = version_of(&fixture_card());
        let written = block_on(apply(
            &mut store,
            &version_key(),
            card_number(),
            &token,
            record,
        ))
        .expect("the update is applied")
        .card;
        assert_eq!(written.card_cvv_cd, cvv(747));
        assert_eq!(written.card_num, card_number());
        assert_eq!(store.cards.len(), 1);
        assert_eq!(store.cards.get(&card_number()), Some(&written));
    }

    #[test]
    fn a_case_only_change_of_the_stored_name_passes_the_change_check() {
        // P-COCRDUPC-005: 9300 upper-cases the locked name before comparing.
        let recased = Card {
            card_embossed_name: Alnum::move_from("aniya von"),
            ..fixture_card()
        };
        let mut store = FakeStore::default().with_card(recased);
        let record = update_record(&fixture_card(), &changed_input());
        let token = version_of(&fixture_card());
        assert!(
            block_on(apply(
                &mut store,
                &version_key(),
                card_number(),
                &token,
                record
            ))
            .is_ok()
        );
    }

    #[test]
    fn apply_rejects_a_card_changed_since_the_fetch_and_writes_nothing() {
        // V-COCRDUPC-019.
        let fetched = fixture_card();
        let token = version_of(&fetched);
        let changes = [
            Card {
                card_active_status: Alnum::move_from("N"),
                ..fetched.clone()
            },
            Card {
                card_cvv_cd: cvv(748),
                ..fetched.clone()
            },
            Card {
                card_expiraion_date: Alnum::move_from("2023-03-10"),
                ..fetched.clone()
            },
        ];
        for since in changes {
            let mut store = FakeStore::default().with_card(since.clone());
            let record = update_record(&fetched, &changed_input());
            assert_rejected(
                block_on(apply(
                    &mut store,
                    &version_key(),
                    card_number(),
                    &token,
                    record,
                )),
                (
                    ErrorKind::Conflict,
                    messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                    "CRDNAME",
                ),
                &format!("{since:?}"),
            );
            assert_eq!(store.cards.get(&card_number()), Some(&since));
            assert_eq!(store.calls, ["read_card_for_update"]);
        }
    }

    #[test]
    fn a_failed_lock_writes_nothing() {
        // V-COCRDUPC-018 (P-COCRDUPC-004): any failure of the READ UPDATE.
        let failures = [
            PortError::Contention,
            PortError::NotFound,
            PortError::Unavailable("lock timeout".to_owned()),
        ];
        for failure in failures {
            let mut store = store();
            let case = format!("{failure:?}");
            store.fail_next("read_card_for_update", failure);
            let record = update_record(&fixture_card(), &changed_input());
            let token = version_of(&fixture_card());
            assert_rejected(
                block_on(apply(
                    &mut store,
                    &version_key(),
                    card_number(),
                    &token,
                    record,
                )),
                (
                    ErrorKind::Unexpected,
                    messages::COULD_NOT_LOCK_RECORD_FOR_UPDATE,
                    "ACCTSID",
                ),
                &case,
            );
            assert_eq!(store.cards.get(&card_number()), Some(&fixture_card()));
            assert_eq!(store.calls, ["read_card_for_update"]);
        }
    }

    #[test]
    fn a_failed_rewrite_leaves_the_card_as_it_was() {
        // V-COCRDUPC-020 (P-COCRDUPC-006).
        let mut store = store();
        store.fail_next(
            "rewrite_card",
            PortError::Unavailable("disk full".to_owned()),
        );
        let record = update_record(&fixture_card(), &changed_input());
        let token = version_of(&fixture_card());
        assert_rejected(
            block_on(apply(
                &mut store,
                &version_key(),
                card_number(),
                &token,
                record,
            )),
            (
                ErrorKind::Unexpected,
                messages::UPDATE_OF_RECORD_FAILED,
                "ACCTSID",
            ),
            "rewrite failure",
        );
        assert_eq!(store.cards.get(&card_number()), Some(&fixture_card()));
        assert_eq!(store.calls, ["read_card_for_update", "rewrite_card"]);
    }

    #[test]
    fn update_reads_validates_locks_and_rewrites_in_order() {
        let mut store = store();
        let expected = Card {
            card_expiraion_date: Alnum::move_from("2023-04-09"),
            ..fixture_card()
        };
        assert_eq!(
            block_on(update(&mut store, &version_key(), &changed_input())),
            Ok(CardUpdated {
                card: expected.clone(),
                message: messages::CHANGES_COMMITTED_TO_DATABASE,
            })
        );
        assert_eq!(
            store.calls,
            ["read_card", "read_card_for_update", "rewrite_card"]
        );
        assert_eq!(store.cards.get(&card_number()), Some(&expected));
    }

    #[test]
    fn update_without_a_change_locks_and_writes_nothing() {
        // V-COCRDUPC-009: 200 with the fetched card.
        let mut store = store();
        assert_eq!(
            block_on(update(&mut store, &version_key(), &unchanged_input())),
            Ok(CardUpdated {
                card: fixture_card(),
                message: messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
            })
        );
        assert_eq!(store.calls, ["read_card"]);
    }

    #[test]
    fn update_stops_at_the_first_failing_step_in_audit_order() {
        let stale_token = version_of(&Card {
            card_active_status: Alnum::move_from("N"),
            ..fixture_card()
        });
        let cases = [
            // A key edit (V-COCRDUPC-002) reads nothing.
            (
                changed_with(|input| input.account_id.clear()),
                (
                    ErrorKind::Empty,
                    messages::ACCOUNT_NUMBER_NOT_PROVIDED,
                    "ACCTSID",
                ),
                &[][..],
            ),
            // The fetch (V-COCRDUPC-007) precedes the card-field edits.
            (
                changed_with(|input| {
                    input.card_number = MISSING_CARD.to_owned();
                    input.embossed_name.clear();
                }),
                (
                    ErrorKind::NotFound,
                    messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION,
                    "ACCTSID",
                ),
                &["read_card"][..],
            ),
            // A card-field edit (V-COCRDUPC-010) precedes the change check.
            (
                changed_with(|input| {
                    input.embossed_name.clear();
                    input.version = stale_token.clone();
                }),
                (
                    ErrorKind::Empty,
                    messages::CARD_NAME_NOT_PROVIDED,
                    "CRDNAME",
                ),
                &["read_card"][..],
            ),
            // The change check (V-COCRDUPC-019) runs on the locked record.
            (
                changed_with(|input| input.version = stale_token.clone()),
                (
                    ErrorKind::Conflict,
                    messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                    "CRDNAME",
                ),
                &["read_card", "read_card_for_update"][..],
            ),
        ];
        for (input, expected, calls) in cases {
            let mut store = store();
            let case = format!("{input:?}");
            assert_rejected(
                block_on(update(&mut store, &version_key(), &input)),
                expected,
                &case,
            );
            assert_eq!(store.calls, calls, "{case}");
            assert_eq!(
                store.cards.get(&card_number()),
                Some(&fixture_card()),
                "{case}"
            );
        }
    }

    #[test]
    fn the_update_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = store();
        let key = version_key();
        let input = changed_input();
        let future = update(&mut store, &key, &input);
        assert_send(&future);
        assert!(block_on(future).is_ok());
    }

    #[test]
    fn input_debug_masks_the_card_number() {
        let shown = format!("{:?}", changed_input());
        assert!(shown.contains("************5740"), "{shown}");
        assert!(!shown.contains(CARD), "{shown}");
    }
}
