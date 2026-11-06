//! `COCRDSLC` (CCDL), the credit card view: `GET /cards/{id}?account_id=`.
//!
//! The 3270 dialogue receives the account and card numbers
//! (`2100-RECEIVE-MAP`), edits them (`2200-EDIT-MAP-INPUTS`,
//! `2210-EDIT-ACCOUNT`, `2220-EDIT-CARD`) and, when both pass, reads the card
//! (`9000-READ-DATA`, `9100-GETCARD-BYACCTCARD`) [app/cbl/COCRDSLC.cbl:L582-778].
//! One request performs that turn: the card number is the `{id}` path segment
//! and the account number the `account_id` query parameter, an absent value
//! being a blank account (E-55). Entry from the card list passes the same
//! edits (P-COCRDSLC-016). [`view`] runs [`edit_map_inputs`] and [`read_data`]
//! in that order; each step is public for an adapter that sequences them
//! itself.
//!
//! # Outcomes
//!
//! | Outcome | Kind | BMS field | Rows |
//! |---|---|---|---|
//! | Account blank, `*` or zeros; not 11 digits | `Empty`, `Invalid` | `ACCTSID` | V-COCRDSLC-001, V-COCRDSLC-002 |
//! | Card blank, `*` or zeros; not 16 digits | `Empty`, `Invalid` | `CARDSID` | V-COCRDSLC-003, V-COCRDSLC-004 |
//! | Both keys blank | `Empty` | `ACCTSID` | V-COCRDSLC-005 |
//! | Card not on file; card read failed | `NotFound`, `Unexpected` | `ACCTSID` | V-COCRDSLC-006, V-COCRDSLC-007 |
//! | Card found | none: [`CardView`], 200 | — | P-COCRDSLC-006, P-COCRDSLC-010 |
//!
//! Both key edits always run, and each moves its text only
//! `IF WS-RETURN-MSG-OFF` [app/cbl/COCRDSLC.cbl:L656] [app/cbl/COCRDSLC.cbl:L668]
//! [app/cbl/COCRDSLC.cbl:L696] [app/cbl/COCRDSLC.cbl:L709], so the account's
//! text stands when both fail; [`ReturnMsg`] is that accumulator for an
//! adapter that runs [`edit_account`] and [`edit_card`] one by one. Any key
//! error skips the read (P-COCRDSLC-013).
//!
//! The read is by card number alone: the `MOVE CC-ACCT-ID-N TO
//! WS-CARD-RID-ACCT-ID` is commented out [app/cbl/COCRDSLC.cbl:L739], so the
//! account entered is edited but never compared with the card's, and a card of
//! another account is shown (E-55). `9150-GETCARD-BYACCT`, the read through the
//! `CARDAIX` path, is performed by no statement and is not carried
//! (P-COCRDSLC-017, P-COCRDSLC-021, E-67): an account without a card number is
//! answered by the card edit, never by an account search. The other paragraphs
//! no path reaches are not carried either (P-COCRDSLC-018, P-COCRDSLC-022,
//! P-COCRDSLC-024, P-COCRDSLC-025); the screen mechanics of
//! `1000-SEND-MAP` (P-COCRDSLC-008 to P-COCRDSLC-012) become the HTTP response.

use std::fmt;

use super::card_update::{CardKeys, record_version};
use super::{ReturnMsg, VersionKey, VersionToken};
use crate::error::{DomainError, ErrorKind};
use crate::messages;
use crate::numeric::{AccountId, Alnum, CardNumber};
use crate::ports::{CardReader, PortError};

/// The card record [`CardView::card`] carries, re-exported from
/// [`crate::entities`] for the API adapter, which maps it to its response.
pub use crate::entities::Card;

/// `ACCTSID`, the account number of the `COCRDSL` map
/// [app/cpy-bms/COCRDSL.CPY:L55-60].
const FIELD_ACCTSID: &str = "ACCTSID";

/// `CARDSID`, the card number of the `COCRDSL` map
/// [app/cpy-bms/COCRDSL.CPY:L61-66].
const FIELD_CARDSID: &str = "CARDSID";

/// `CC-ACCT-ID PIC X(11)`, the receiver of `ACCTSIDI X(11)`
/// [app/cpy/CVCRD01Y.cpy:L34-36].
const ACCOUNT_ID_LEN: usize = 11;

/// `CC-CARD-NUM PIC X(16)`, the receiver of `CARDSIDI X(16)`
/// [app/cpy/CVCRD01Y.cpy:L37-39].
const CARD_NUM_LEN: usize = 16;

/// `ERROR-OPNAME` of a failed card read [app/cbl/COCRDSLC.cbl:L767].
const ERROR_OPNAME_READ: &str = "READ";

/// `LIT-CARDFILENAME`, the `ERROR-FILE` of a failed card read
/// [app/cbl/COCRDSLC.cbl:L187-188] [app/cbl/COCRDSLC.cbl:L768].
const LIT_CARDFILENAME: &str = "CARDDAT ";

/// The CICS `RESP` a card read failure other than `NOTFND` reports: `IOERR`,
/// the code E-44 proposes for a database failure, which has no CICS code.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 proposes alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// One `GET /cards/{id}` request: the `COCRDSL` map input as entered.
///
/// `card_number` is the `{id}` path segment (`CARDSIDI X(16)`) and
/// `account_id` the `account_id` query parameter (`ACCTSIDI X(11)`), each
/// exactly as received (E-55). `None` is a key not entered.
///
/// `Debug` shows the card number masked as [`CardNumber`] masks it.
#[derive(Clone, PartialEq, Eq)]
pub struct CardViewInput {
    /// `ACCTSIDI X(11)`.
    pub account_id: Option<String>,
    /// `CARDSIDI X(16)`.
    pub card_number: Option<String>,
}

impl fmt::Debug for CardViewInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked = self.card_number.as_deref().map(CardNumber::move_from);
        formatter
            .debug_struct("CardViewInput")
            .field("account_id", &self.account_id)
            .field("card_number", &masked)
            .finish()
    }
}

/// The card a successful request shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardView {
    /// The `CARDDAT` record read by the card number: `1200-SETUP-SCREEN-VARS`
    /// shows its embossed name, expiry month and year and active status with
    /// the two keys (P-COCRDSLC-006). Its account may differ from the one
    /// entered (E-55). The verification code is part of the record, not of
    /// the screen; the API response leaves it out.
    pub card: Card,
    /// The token `PUT /cards/{id}` echoes back:
    /// [`card_update::record_version`](super::card_update::record_version) of
    /// [`CardView::card`] under the server's [`VersionKey`]. The digest is
    /// keyed, so the token discloses nothing about the verification code the
    /// response leaves out.
    pub version: VersionToken,
    /// The information line of a found card,
    /// [`DISPLAYING_REQUESTED_DETAILS`](messages::DISPLAYING_REQUESTED_DETAILS)
    /// with its three leading spaces (P-COCRDSLC-010).
    pub message: &'static str,
}

/// The texts and BMS field of one key edit.
struct KeyEditTexts {
    /// The map field the edit's messages concern.
    field: &'static str,
    /// The text of `FLG-*FILTER-BLANK`.
    not_provided: &'static str,
    /// The text of `FLG-*FILTER-NOT-OK`.
    not_numeric: &'static str,
}

/// `2210-EDIT-ACCOUNT` [app/cbl/COCRDSLC.cbl:L647-684].
const ACCOUNT_EDIT: KeyEditTexts = KeyEditTexts {
    field: FIELD_ACCTSID,
    not_provided: messages::ACCOUNT_NUMBER_NOT_PROVIDED,
    not_numeric: messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT,
};

/// `2220-EDIT-CARD` [app/cbl/COCRDSLC.cbl:L685-725].
const CARD_EDIT: KeyEditTexts = KeyEditTexts {
    field: FIELD_CARDSID,
    not_provided: messages::CARD_NUMBER_NOT_PROVIDED,
    not_numeric: messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
};

/// The flag a failing key edit sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRejection {
    /// `FLG-ACCTFILTER-BLANK` or `FLG-CARDFILTER-BLANK`: low-values, spaces or
    /// zeros.
    Blank,
    /// `FLG-ACCTFILTER-NOT-OK` or `FLG-CARDFILTER-NOT-OK`: not numeric.
    NotOk,
}

impl KeyRejection {
    /// The kind and text of this rejection by the edit `texts` describes.
    const fn outcome(self, texts: &KeyEditTexts) -> (ErrorKind, &'static str) {
        match self {
            Self::Blank => (ErrorKind::Empty, texts.not_provided),
            Self::NotOk => (ErrorKind::Invalid, texts.not_numeric),
        }
    }

    /// This rejection as the request's error, at the edit's field.
    fn error(self, texts: &KeyEditTexts) -> DomainError {
        let (kind, message) = self.outcome(texts);
        DomainError::new(kind, message).with_field(texts.field)
    }
}

/// COCRDSLC 2200-EDIT-MAP-INPUTS, 2210-EDIT-ACCOUNT, 2220-EDIT-CARD —
/// P-COCRDSLC-002, P-COCRDSLC-003, P-COCRDSLC-015
///
/// The entry `raw` moved into its `X(N)` receiver and classified as the edits
/// classify it: `'*'` or `SPACES` becomes `LOW-VALUES`
/// [app/cbl/COCRDSLC.cbl:L615-627]; `LOW-VALUES`, `SPACES` or zeros is not
/// provided [app/cbl/COCRDSLC.cbl:L651-653] [app/cbl/COCRDSLC.cbl:L691-693];
/// otherwise the receiver must be `NUMERIC`, `N` digits, since the `MOVE` pads
/// a shorter entry with spaces.
///
/// The zero test reads the `9(N)` redefine, which is unpinned for a receiver
/// holding a non-digit (E-47, at `halt`). Such an entry gets the outcome E-47
/// decides and V-COCRDSLC-002 records for `0000000005A`: the zero test is
/// false, so the `NUMERIC` test rejects it with its own text.
///
/// An entry longer than the receiver has no 3270 counterpart, the map field
/// holding `N` characters. It is rejected as not numeric rather than cut, so a
/// path segment naming a longer number never reads the card its first `N`
/// characters name.
fn classify<const N: usize>(raw: Option<&str>) -> Result<Alnum<N>, KeyRejection> {
    let text = raw.unwrap_or_default();
    if text.len() > N {
        return Err(KeyRejection::NotOk);
    }
    let field = Alnum::<N>::move_from(text);
    let star = field.cobol_cmp(b"*").is_eq();
    let zeros = field.as_bytes().iter().all(|&byte| byte == b'0');
    if star || zeros || field.is_blank() {
        Err(KeyRejection::Blank)
    } else if field.is_numeric() {
        Ok(field)
    } else {
        Err(KeyRejection::NotOk)
    }
}

/// COCRDSLC 2210-EDIT-ACCOUNT — P-COCRDSLC-002, V-COCRDSLC-001,
/// V-COCRDSLC-002
fn account_key(raw: Option<&str>) -> Result<AccountId, KeyRejection> {
    // Eleven digits always fit `9(11)`; the mapping only keeps the conversion
    // free of a panic.
    classify::<ACCOUNT_ID_LEN>(raw)?
        .as_text()
        .parse()
        .map_err(|_| KeyRejection::NotOk)
}

/// COCRDSLC 2220-EDIT-CARD — P-COCRDSLC-003, V-COCRDSLC-003, V-COCRDSLC-004
fn card_key(raw: Option<&str>) -> Result<CardNumber, KeyRejection> {
    classify::<CARD_NUM_LEN>(raw).map(CardNumber::from_alnum)
}

/// The value of a key edit, recording its rejection in `msg`
/// `IF WS-RETURN-MSG-OFF`.
fn recorded<T>(
    edit: Result<T, KeyRejection>,
    texts: &KeyEditTexts,
    msg: &mut ReturnMsg,
) -> Option<T> {
    match edit {
        Ok(value) => Some(value),
        Err(rejection) => {
            let (_, message) = rejection.outcome(texts);
            msg.set_if_off(message, texts.field);
            None
        }
    }
}

/// COCRDSLC 2210-EDIT-ACCOUNT — P-COCRDSLC-002, V-COCRDSLC-001,
/// V-COCRDSLC-002
///
/// The account entered, when it is 11 digits that are not all zero
/// [app/cbl/COCRDSLC.cbl:L675-678]. Otherwise `None`, and `msg` takes
/// [`ACCOUNT_NUMBER_NOT_PROVIDED`](messages::ACCOUNT_NUMBER_NOT_PROVIDED) for
/// an absent, blank, `*` or zero entry, or
/// [`ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT`](messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT)
/// for any other, at `ACCTSID`, only if it holds no message yet.
#[must_use]
pub fn edit_account(raw: Option<&str>, msg: &mut ReturnMsg) -> Option<AccountId> {
    recorded(account_key(raw), &ACCOUNT_EDIT, msg)
}

/// COCRDSLC 2220-EDIT-CARD — P-COCRDSLC-003, V-COCRDSLC-003, V-COCRDSLC-004
///
/// The card number entered, when it is 16 digits that are not all zero
/// [app/cbl/COCRDSLC.cbl:L716-719]. Otherwise `None`, and `msg` takes
/// [`CARD_NUMBER_NOT_PROVIDED`](messages::CARD_NUMBER_NOT_PROVIDED) for an
/// absent, blank, `*` or zero entry, or
/// [`CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT`](messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT)
/// for any other, at `CARDSID`, only if it holds no message yet.
#[must_use]
pub fn edit_card(raw: Option<&str>, msg: &mut ReturnMsg) -> Option<CardNumber> {
    recorded(card_key(raw), &CARD_EDIT, msg)
}

/// COCRDSLC 2200-EDIT-MAP-INPUTS, 2210-EDIT-ACCOUNT, 2220-EDIT-CARD —
/// P-COCRDSLC-002, P-COCRDSLC-003, P-COCRDSLC-013, P-COCRDSLC-015,
/// V-COCRDSLC-001 to V-COCRDSLC-005
///
/// The key edits of one request [app/cbl/COCRDSLC.cbl:L608-641]: the account,
/// then the card number, as [`edit_account`] and [`edit_card`] edit them. The
/// account's text stands when both fail; when both keys are blank (or zero),
/// `SET NO-SEARCH-CRITERIA-RECEIVED` replaces the account's text with
/// [`NO_INPUT_RECEIVED`](messages::NO_INPUT_RECEIVED) at `ACCTSID`
/// unconditionally [app/cbl/COCRDSLC.cbl:L637-640].
///
/// # Errors
///
/// The first failing key edit, as a [`DomainError`] of kind
/// [`ErrorKind::Empty`] (a key not provided, or both keys blank) or
/// [`ErrorKind::Invalid`] (a key not 11 or 16 digits), at `ACCTSID` or
/// `CARDSID`.
pub fn edit_map_inputs(
    account_id: Option<&str>,
    card_number: Option<&str>,
) -> Result<CardKeys, DomainError> {
    match (account_key(account_id), card_key(card_number)) {
        (Ok(account_id), Ok(card_number)) => Ok(CardKeys {
            account_id,
            card_number,
        }),
        (Err(KeyRejection::Blank), Err(KeyRejection::Blank)) => {
            Err(DomainError::empty(messages::NO_INPUT_RECEIVED).with_field(FIELD_ACCTSID))
        }
        (Err(first), _) => Err(first.error(&ACCOUNT_EDIT)),
        (Ok(_), Err(first)) => Err(first.error(&CARD_EDIT)),
    }
}

/// COCRDSLC 9000-READ-DATA, 9100-GETCARD-BYACCTCARD — P-COCRDSLC-004,
/// P-COCRDSLC-005, P-COCRDSLC-016, V-COCRDSLC-006, V-COCRDSLC-007
///
/// CICS `READ` of `CARDDAT` by `card_number` alone, without a lock
/// [app/cbl/COCRDSLC.cbl:L736-772]. The account entered takes no part: the
/// `MOVE` of it into the record key is commented out (E-55).
///
/// # Errors
///
/// - [`ErrorKind::NotFound`] with
///   [`DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION`](messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION)
///   at `ACCTSID` when no card has the number (`NOTFND`). The cursor goes to
///   `ACCTSID` because the read sets both keys' `NOT-OK` flags and the
///   account's is tested first [app/cbl/COCRDSLC.cbl:L515-524].
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
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => DomainError::unexpected(messages::file_error_message(
                ERROR_OPNAME_READ,
                LIT_CARDFILENAME,
                RESP_IOERR,
                RESP2_IOERR,
            ))
            .with_field(FIELD_ACCTSID),
        })
}

/// COCRDSLC 2000-PROCESS-INPUTS, 9000-READ-DATA, 1200-SETUP-SCREEN-VARS —
/// P-COCRDSLC-002 to P-COCRDSLC-006, P-COCRDSLC-010, P-COCRDSLC-013 to
/// P-COCRDSLC-016, V-COCRDSLC-001 to V-COCRDSLC-007
///
/// The whole `GET /cards/{id}`: [`edit_map_inputs`], then, when both keys
/// pass, [`read_data`] by the card number. The card found is shown with its
/// [`VersionToken`], computed under `key`, and the information line
/// [`DISPLAYING_REQUESTED_DETAILS`](messages::DISPLAYING_REQUESTED_DETAILS).
/// Nothing is written; the caller owns the read-only unit of work.
///
/// # Errors
///
/// The error of the first step that fails, as that step documents it: the key
/// edits (V-COCRDSLC-001 to V-COCRDSLC-005, no read), then the read
/// (V-COCRDSLC-006, V-COCRDSLC-007), then the token, as
/// [`record_version`] documents it.
pub async fn view<S: CardReader>(
    store: &mut S,
    key: &VersionKey,
    input: CardViewInput,
) -> Result<CardView, DomainError> {
    let CardViewInput {
        account_id,
        card_number,
    } = input;
    let keys = edit_map_inputs(account_id.as_deref(), card_number.as_deref())?;
    let card = read_data(store, keys.card_number).await?;
    let version = record_version(key, &card)?;
    Ok(CardView {
        card,
        version,
        message: messages::DISPLAYING_REQUESTED_DETAILS,
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::future::Future;

    use super::{CardView, CardViewInput, edit_account, edit_card, edit_map_inputs, view};
    use crate::entities::Card;
    use crate::error::{DomainError, ErrorKind};
    use crate::messages;
    use crate::numeric::{AccountId, Alnum, CardNumber, Cvv};
    use crate::ports::PortError;
    use crate::rules::card_update::{CardKeys, record_version};
    use crate::rules::test_support::{FakeStore, block_on, version_key};
    use crate::rules::{ReturnMsg, VersionToken};

    /// The first fixture card, of account 00000000050 (P-CVACT02Y-009)
    /// [app/data/ASCII/carddata.txt:L1].
    const CARD: &str = "0500024453765740";

    /// The account of [`CARD`].
    const ACCOUNT: &str = "00000000050";

    /// The second fixture card, of account 00000000027
    /// [app/data/ASCII/carddata.txt:L2].
    const OTHER_CARD: &str = "0683586198171516";

    /// A card number no fixture card has (P-CVACT02Y-010).
    const MISSING_CARD: &str = "9999999999999999";

    /// `ACCTSID` and `CARDSID`, as the validation audit names them.
    const ACCTSID: &str = "ACCTSID";
    const CARDSID: &str = "CARDSID";

    fn account(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn fixture_card(number: &str, account_id: u64, cvv: u16, name: &str, expiry: &str) -> Card {
        Card {
            card_num: CardNumber::move_from(number),
            card_acct_id: account(account_id),
            card_cvv_cd: Cvv::new(cvv).expect("a three-digit code"),
            card_embossed_name: Alnum::move_from(name),
            card_expiraion_date: Alnum::move_from(expiry),
            card_active_status: Alnum::move_from("Y"),
        }
    }

    /// [`CARD`] as the fixture holds it.
    fn first_card() -> Card {
        fixture_card(CARD, 50, 747, "Aniya Von", "2023-03-09")
    }

    /// [`OTHER_CARD`] as the fixture holds it.
    fn second_card() -> Card {
        fixture_card(OTHER_CARD, 27, 567, "Ward Jones", "2025-07-13")
    }

    /// A store holding the first two fixture cards.
    fn store() -> FakeStore {
        FakeStore::default()
            .with_card(first_card())
            .with_card(second_card())
    }

    /// The token `PUT /cards/{id}` recomputes for `card` under the tests'
    /// server key.
    fn version_of(card: &Card) -> VersionToken {
        record_version(&version_key(), card).expect("the token of a card")
    }

    fn input(account_id: Option<&str>, card_number: Option<&str>) -> CardViewInput {
        CardViewInput {
            account_id: account_id.map(str::to_owned),
            card_number: card_number.map(str::to_owned),
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

    /// The outcome of the key edits on `account_id` and `card_number`, computed
    /// as the COBOL computes it: both per-field edits into one `ReturnMsg`, then
    /// the cross edit.
    fn by_return_msg(
        account_id: Option<&str>,
        card_number: Option<&str>,
    ) -> Result<CardKeys, (String, &'static str)> {
        let mut card_alone = ReturnMsg::default();
        let card_blank = edit_card(card_number, &mut card_alone).is_none()
            && card_alone.message() == Some(messages::CARD_NUMBER_NOT_PROVIDED);

        let mut msg = ReturnMsg::default();
        let account_id = edit_account(account_id, &mut msg);
        let account_blank = msg.message() == Some(messages::ACCOUNT_NUMBER_NOT_PROVIDED);
        let card_number = edit_card(card_number, &mut msg);
        if account_blank && card_blank {
            return Err((messages::NO_INPUT_RECEIVED.to_owned(), ACCTSID));
        }
        match (account_id, card_number, msg.into_parts()) {
            (Some(account_id), Some(card_number), None) => Ok(CardKeys {
                account_id,
                card_number,
            }),
            (_, _, Some(first)) => Err(first),
            other => panic!("the edits disagree with their message: {other:?}"),
        }
    }

    #[test]
    fn key_edits_keep_the_first_text_and_replace_two_blank_keys() {
        let account_empty = (
            ErrorKind::Empty,
            messages::ACCOUNT_NUMBER_NOT_PROVIDED,
            ACCTSID,
        );
        let account_invalid = (
            ErrorKind::Invalid,
            messages::ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT,
            ACCTSID,
        );
        let card_empty = (
            ErrorKind::Empty,
            messages::CARD_NUMBER_NOT_PROVIDED,
            CARDSID,
        );
        let card_invalid = (
            ErrorKind::Invalid,
            messages::CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
            CARDSID,
        );
        let no_input = (ErrorKind::Empty, messages::NO_INPUT_RECEIVED, ACCTSID);
        let nul_account = "\0".repeat(11);
        let cases = [
            // V-COCRDSLC-001: absent, empty, spaces, '*', low-values, zeros.
            (None, Some(CARD), account_empty),
            (Some(""), Some(CARD), account_empty),
            (Some("           "), Some(CARD), account_empty),
            (Some("*"), Some(CARD), account_empty),
            (Some(nul_account.as_str()), Some(CARD), account_empty),
            (Some("00000000000"), Some(CARD), account_empty),
            // V-COCRDSLC-002, with E-47's outcome for non-digits, and short,
            // signed and over-long entries.
            (Some("0000000005A"), Some(CARD), account_invalid),
            (Some("50"), Some(CARD), account_invalid),
            (Some("-0000000050"), Some(CARD), account_invalid),
            (Some("000000000500"), Some(CARD), account_invalid),
            // V-COCRDSLC-003: the account-only search of 9150 is not carried.
            (Some(ACCOUNT), None, card_empty),
            (Some(ACCOUNT), Some("*"), card_empty),
            (Some(ACCOUNT), Some("0000000000000000"), card_empty),
            // V-COCRDSLC-004.
            (Some(ACCOUNT), Some("050002445376574X"), card_invalid),
            (Some(ACCOUNT), Some("05000244537657400"), card_invalid),
            (Some(ACCOUNT), Some("5740"), card_invalid),
            // V-COCRDSLC-005: the SET replaces the account's text.
            (None, None, no_input),
            (Some(" "), Some("*"), no_input),
            (Some("00000000000"), Some("0000000000000000"), no_input),
            // The account edit runs first, so its text stands.
            (Some("0000000005A"), None, account_invalid),
            (None, Some("050002445376574X"), account_empty),
            (
                Some("0000000005A"),
                Some("050002445376574X"),
                account_invalid,
            ),
        ];
        for (account_id, card_number, expected) in cases {
            let case = format!("{account_id:?} {card_number:?}");
            let result = edit_map_inputs(account_id, card_number);
            let (_, message, field) = expected;
            assert_eq!(
                by_return_msg(account_id, card_number),
                Err((message.to_owned(), field)),
                "{case}"
            );
            assert_rejected(result, expected, &case);

            let mut store = store();
            let viewed = block_on(view(
                &mut store,
                &version_key(),
                input(account_id, card_number),
            ));
            assert_rejected(viewed, expected, &case);
            assert!(store.calls.is_empty(), "{case}: a key error skips the read");
        }
    }

    #[test]
    fn valid_keys_pass_every_edit() {
        // P-COCRDSLC-002, P-COCRDSLC-003
        let keys = CardKeys {
            account_id: account(50),
            card_number: CardNumber::move_from(CARD),
        };
        assert_eq!(edit_map_inputs(Some(ACCOUNT), Some(CARD)), Ok(keys));
        assert_eq!(by_return_msg(Some(ACCOUNT), Some(CARD)), Ok(keys));
    }

    #[test]
    fn a_set_message_is_kept_by_later_edits() {
        // WS-RETURN-MSG-OFF guard of 2210 and 2220.
        let mut msg = ReturnMsg::default();
        msg.set_if_off(messages::NO_INPUT_RECEIVED, ACCTSID);
        assert_eq!(edit_account(Some("0000000005A"), &mut msg), None);
        assert_eq!(edit_card(None, &mut msg), None);
        assert_eq!(msg.message(), Some(messages::NO_INPUT_RECEIVED));
        assert_eq!(msg.field(), Some(ACCTSID));

        let mut msg = ReturnMsg::default();
        assert_eq!(edit_account(Some(ACCOUNT), &mut msg), Some(account(50)));
        assert_eq!(
            edit_card(Some(OTHER_CARD), &mut msg),
            Some(CardNumber::move_from(OTHER_CARD))
        );
        assert!(msg.is_off(), "valid keys set no message");
    }

    #[test]
    fn a_fixture_card_is_shown_with_its_token_and_information_line() {
        // P-COCRDSLC-004, P-COCRDSLC-006, P-COCRDSLC-010, P-COCRDSLC-016
        let mut store = store();
        let shown = block_on(view(
            &mut store,
            &version_key(),
            input(Some(ACCOUNT), Some(CARD)),
        ))
        .expect("the fixture card is on file");
        let expected = CardView {
            card: first_card(),
            version: version_of(&first_card()),
            message: messages::DISPLAYING_REQUESTED_DETAILS,
        };
        assert_eq!(shown, expected);
        assert_eq!(store.calls, ["read_card"]);
    }

    #[test]
    fn the_token_is_the_card_update_token() {
        // P-COCRDUPC-022: the GET token is the one PUT /cards/{id} recomputes.
        let mut store = store();
        let shown = block_on(view(
            &mut store,
            &version_key(),
            input(Some("00000000027"), Some(OTHER_CARD)),
        ))
        .expect("the fixture card is on file");
        assert_eq!(shown.version, version_of(&second_card()));
        assert_ne!(shown.version, version_of(&first_card()));
        assert_eq!(
            VersionToken::parse(shown.version.as_str()),
            Some(shown.version.clone()),
            "16 lower-case hex digits"
        );
    }

    #[test]
    fn the_account_entered_is_not_compared_with_the_card() {
        // E-55, P-COCRDSLC-016: the read is by card number alone.
        for entered in ["00000000001", "99999999999", "00000000027"] {
            let mut store = store();
            let shown = block_on(view(
                &mut store,
                &version_key(),
                input(Some(entered), Some(CARD)),
            ))
            .expect("the card is read by its number");
            assert_eq!(shown.card, first_card(), "{entered}");
            assert_eq!(shown.card.card_acct_id, account(50), "{entered}");
        }
    }

    #[test]
    fn an_unknown_card_is_not_found() {
        // V-COCRDSLC-006, P-COCRDSLC-004
        let mut store = store();
        let result = block_on(view(
            &mut store,
            &version_key(),
            input(Some(ACCOUNT), Some(MISSING_CARD)),
        ));
        let expected = (
            ErrorKind::NotFound,
            messages::DID_NOT_FIND_CARDS_FOR_THIS_SEARCH_CONDITION,
            ACCTSID,
        );
        assert_rejected(result, expected, MISSING_CARD);
        assert_eq!(store.calls, ["read_card"]);
    }

    #[test]
    fn any_other_read_failure_is_the_file_error_text() {
        // V-COCRDSLC-007, P-COCRDSLC-005, E-44
        let text = messages::file_error_message("READ", "CARDDAT ", 17, 0);
        let failures = [
            PortError::Unavailable("connection reset".to_owned()),
            PortError::Contention,
            PortError::Duplicate,
            PortError::Restricted,
        ];
        for failure in failures {
            let case = format!("{failure:?}");
            let mut store = store();
            store.fail_next("read_card", failure);
            let result = block_on(view(
                &mut store,
                &version_key(),
                input(Some(ACCOUNT), Some(CARD)),
            ));
            assert_rejected(result, (ErrorKind::Unexpected, &text, ACCTSID), &case);
            assert_eq!(store.calls, ["read_card"], "{case}");
        }
    }

    #[test]
    fn the_view_future_is_send() {
        // Adapters run the rule on a multi-threaded runtime.
        fn assert_send<F: Future + Send>(future: F) -> F {
            future
        }
        let mut store = store();
        let key = version_key();
        let future = assert_send(view(&mut store, &key, input(Some(ACCOUNT), Some(CARD))));
        assert!(block_on(future).is_ok());
    }

    #[test]
    fn debug_masks_the_card_number() {
        let shown = format!("{:?}", input(Some(ACCOUNT), Some(CARD)));
        assert!(shown.contains("************5740"), "{shown}");
        assert!(!shown.contains(CARD), "{shown}");
        assert!(shown.contains(ACCOUNT), "{shown}");
    }
}
