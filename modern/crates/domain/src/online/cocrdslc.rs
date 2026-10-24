//! COCRDSLC, the card detail: the rules of endpoint 8, `GET /api/v1/cards/view` (transaction
//! `CCDL`, map `CCRDSLA`, route `/cards/view`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COCRDSLC.md`: the edits of
//! `2200-EDIT-MAP-INPUTS`, `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD` with their message
//! precedence and cursor field (COCRDSLC-006 to COCRDSLC-013), the card read of
//! `9000-READ-DATA` and `9100-GETCARD-BYACCTCARD` (COCRDSLC-014 to COCRDSLC-017), and the
//! response `1200-SETUP-SCREEN-VARS` builds (COCRDSLC-018 to COCRDSLC-023). The `client` rows
//! belong to the screen: entry, F3 and the transfers (COCRDSLC-001 to COCRDSLC-004), the
//! expiry month and year parts (COCRDSLC-024), the prompt on the information row
//! (COCRDSLC-025), the re-read with the response's handle (COCRDSLC-026) and the colours of the
//! inputs (COCRDSLC-027).
//!
//! # Flow
//!
//! `api::services::cocrdslc` calls [`edit`] on the raw query text, which yields the account and
//! a [`CardKey`]: a typed card number, or the `cards.card_ref` handle of a COCRDLI list row. The
//! service reads the card by that key alone, maps the store's answer into a [`CardLookup`] and
//! calls [`view`] with the verified token's [`UserType`], which decides the response or the
//! failure. This module does no I/O.
//!
//! # Disclosure
//!
//! Endpoint 8 is the single operation that may return all 16 digits of a card number (AAP
//! 0.3.4.8). [`disclose`] decides it from the verified `usr_type` alone: the full number for
//! `A` (COCRDSLC-022), the redacted form `************` and the last four digits for every
//! other type (COCRDSLC-023, `E32`). No request field reaches that choice (AAP 0.5.1), and the
//! rule's owner clause never grants disclosure, because no user is linked to a card.
//!
//! # Quirks kept
//!
//! The card is read by its number alone and its own account is never compared with the account
//! entered, so a card of another account is shown under the account the request names
//! (COCRDSLC-014, COCRDSLC-018, `X-COCRDSLC-1`). The texts the program declares and never moves
//! are never returned: `'Account number must be a non zero 11 digit number'`, `'Card number if
//! supplied must be a 16 digit number'`, `'Did not find this account in cards database'`,
//! `'Error reading Card Data File'`, `'Looks Good.... so far'` and the F3 exit text
//! (COCRDSLC-028 to COCRDSLC-031, `X-COCRDSLC-2`); `9150-GETCARD-BYACCT` is never performed and
//! is not ported.
//!
//! # Halts
//!
//! - `HALTED(H4)`: the `RESP` and `RESP2` fields of the file-error text hold CICS values that
//!   have no counterpart; they keep the spaces of their `VALUE SPACES`, which is no parity
//!   result and never carries a key value or database text (COCRDSLC-016, COCRDSLC-017).
//! - Halt candidate `X-COCRDSLC-H1`: which inputs that are neither blank nor all digits the
//!   COBOL rejects depends on compiler options outside the repository. [`edit`] takes the
//!   program's evident path, the class test and its inline message, which is no parity result
//!   (COCRDSLC-011, COCRDSLC-012).
//! - Halt candidate `X-COCRDSLC-H2`: the first-entry test reads working storage before the
//!   received COMMAREA is moved in (COCRDSLC-005). Every request here is edited, the outcome on
//!   storage that does not match, which is also the outcome of the intended test.
//!
//! # Totality
//!
//! [`edit`] takes text of any length or encoding and returns a value or a [`DetailFailure`];
//! nothing indexes, slices or panics, and no failure holds any part of the input (Total Parsers
//! Over Untrusted Input, Cardholder Data Redaction).

use core::fmt;

use chrono::NaiveDate;
use copybook::{PicU, PicX};

use super::cocrdlic::CardRef;
use super::cosgn00c::UserType;
use crate::records::card::CardRecord;
use crate::sensitive::{CardNumber, DisclosedCardNumber};

/// Width of `ACCTSIDI PIC X(11)` and `CC-ACCT-ID PIC X(11)` (`COCRDSL.CPY`, `CVCRD01Y`).
const ACCT_WIDTH: usize = 11;

/// Width of `CARDSIDI PIC X(16)` and `CC-CARD-NUM PIC X(16)` (`COCRDSL.CPY`, `CVCRD01Y`).
const CARD_WIDTH: usize = 16;

/// `FOUND-CARDS-FOR-ACCOUNT` of `WS-INFO-MSG PIC X(40)` (`COCRDSLC.cbl:129-130`), the
/// information message of every found card, three leading spaces included (COCRDSLC-020).
pub const FOUND_CARDS_FOR_ACCOUNT: &str = "   Displaying requested details";

/// `WS-PROMPT-FOR-ACCT` (`COCRDSLC.cbl:138-139`): the account input is not supplied
/// (COCRDSLC-009).
pub const PROMPT_FOR_ACCT: &str = "Account number not provided";

/// `WS-PROMPT-FOR-CARD` (`COCRDSLC.cbl:140-141`): the account is valid and no card is supplied
/// (COCRDSLC-010).
pub const PROMPT_FOR_CARD: &str = "Card number not provided";

/// `NO-SEARCH-CRITERIA-RECEIVED` (`COCRDSLC.cbl:142-143`): neither input is supplied; the
/// cross-field edit replaces the account prompt with it (COCRDSLC-008).
pub const NO_SEARCH_CRITERIA_RECEIVED: &str = "No input received";

/// The inline literal `2210-EDIT-ACCOUNT` moves when its class test fails
/// (`COCRDSLC.cbl:670`, COCRDSLC-011); no 88-level holds it.
pub const ACCOUNT_FILTER_INVALID: &str = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";

/// The inline literal `2220-EDIT-CARD` moves when its class test fails (`COCRDSLC.cbl:711`,
/// COCRDSLC-012); also the answer to a request carrying both a card number and a handle
/// (COCRDSLC-007).
pub const CARD_FILTER_INVALID: &str = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

/// `DID-NOT-FIND-ACCTCARD-COMBO` (`COCRDSLC.cbl:153-154`): no card has the number or the handle
/// (COCRDSLC-015). The text embeds no CICS code.
pub const DID_NOT_FIND_ACCTCARD_COMBO: &str = "Did not find cards for this search condition";

/// `WS-FILE-ERROR-MESSAGE` (`COCRDSLC.cbl:102-121`) after a failed `READ` of `CARDDAT`, cut to
/// the 75 bytes of `WS-RETURN-MSG PIC X(75)`: its five trailing `FILLER` spaces are dropped by
/// the move (COCRDSLC-016).
///
/// `HALTED(H4)`: `ERROR-RESP` and `ERROR-RESP2 PIC X(10)`, bytes 49-58 and 66-75, would hold
/// the CICS `RESP` and `RESP2` values, which have no counterpart; they keep the spaces of their
/// `VALUE SPACES` (COCRDSLC-017). The text is kept whole, trailing spaces included, so bytes 1-48
/// and 59-65 stay assertable.
pub const FILE_ERROR: &str = concat!(
    "File Error: ",    // FILLER PIC X(12) VALUE 'File Error: '
    "READ    ",        // ERROR-OPNAME PIC X(8) <- 'READ'
    " on ",            // FILLER PIC X(4) VALUE ' on '
    "CARDDAT  ",       // ERROR-FILE PIC X(9) <- LIT-CARDFILENAME 'CARDDAT '
    " returned RESP ", // FILLER PIC X(15)
    "          ",      // ERROR-RESP PIC X(10): HALTED(H4)
    ",RESP2 ",         // FILLER PIC X(7)
    "          ",      // ERROR-RESP2 PIC X(10): HALTED(H4)
);

const _: () = assert!(FILE_ERROR.len() == 75);
const _: () = assert!(FOUND_CARDS_FOR_ACCOUNT.len() <= 40);
const _: () = assert!(PROMPT_FOR_ACCT.len() <= 75);
const _: () = assert!(PROMPT_FOR_CARD.len() <= 75);
const _: () = assert!(NO_SEARCH_CRITERIA_RECEIVED.len() <= 75);
const _: () = assert!(ACCOUNT_FILTER_INVALID.len() <= 75);
const _: () = assert!(CARD_FILTER_INVALID.len() <= 75);
const _: () = assert!(DID_NOT_FIND_ACCTCARD_COMBO.len() <= 75);

/// The card a request names, after [`edit`]: the program's 16-digit card number, or the handle
/// of a COCRDLI list row, which stands for the number COCRDLIC passes on its `S` selection
/// (COCRDSLC-006). The service reads the card by this key alone (COCRDSLC-014).
///
/// The number is request input, never egress: its `Debug` is the redacted form.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CardKey {
    /// A typed `card_num`: exactly 16 ASCII digits, not all `0` (`FLG-CARDFILTER-ISVALID`).
    ByNumber(CardNumber),
    /// A `card_ref` handle; `2220-EDIT-CARD` does not run for it (COCRDSLC-006).
    ByRef(CardRef),
}

/// One input after its edit.
enum Edit<T> {
    /// Not supplied (`FLG-ACCTFILTER-BLANK`, `FLG-CARDFILTER-BLANK`).
    Blank,
    /// Supplied and valid (`FLG-ACCTFILTER-ISVALID`, `FLG-CARDFILTER-ISVALID`).
    Valid(T),
    /// Supplied and not valid (`FLG-ACCTFILTER-NOT-OK`, `FLG-CARDFILTER-NOT-OK`).
    Invalid,
}

/// Whether a moved input field counts as not supplied: `2200-EDIT-MAP-INPUTS` turns `*` alone
/// and `SPACES` into `LOW-VALUES` (`COCRDSLC.cbl:614-627`), and the edits treat `LOW-VALUES`,
/// `SPACES` and a zero value alike (`COCRDSLC.cbl:651-653,691-693`; COCRDSLC-008 to
/// COCRDSLC-010).
fn not_supplied(field: &[u8]) -> bool {
    let asterisk_alone = matches!(
        field.split_first(),
        Some((b'*', rest)) if rest.iter().all(|&byte| byte == b' ')
    );
    asterisk_alone
        || [b' ', 0_u8, b'0']
            .iter()
            .any(|&fill| field.iter().all(|&byte| byte == fill))
}

/// `2210-EDIT-ACCOUNT` over the account text, padded with spaces to `CC-ACCT-ID PIC X(11)` as
/// the terminal pads the field (COCRDSLC-009, COCRDSLC-011).
fn edit_account(text: &str) -> Edit<PicU<11>> {
    let bytes = text.as_bytes();
    // A text longer than the 11-byte field cannot be what the terminal sent, and a MOVE would
    // drop its tail; it fails the class test rather than being cut, so a request never names an
    // account other than the one typed.
    if bytes.len() > ACCT_WIDTH {
        return Edit::Invalid;
    }
    let field = PicX::<ACCT_WIDTH>::moved_from(bytes);
    if not_supplied(field.as_bytes()) {
        return Edit::Blank;
    }
    // HALTED(X-COCRDSLC-H1): for text that is not all digits the zero test runs on invalid
    // zoned data before this class test; this is the program's evident path, no parity result.
    PicU::decode(field.as_bytes()).map_or(Edit::Invalid, Edit::Valid)
}

/// `2220-EDIT-CARD` over the card text, padded with spaces to `CC-CARD-NUM PIC X(16)`
/// (COCRDSLC-010, COCRDSLC-012).
fn edit_card(text: &str) -> Edit<CardNumber> {
    let bytes = text.as_bytes();
    // As for the account: a text longer than the field is refused, never cut into another card.
    if bytes.len() > CARD_WIDTH {
        return Edit::Invalid;
    }
    let field = PicX::<CARD_WIDTH>::moved_from(bytes);
    if not_supplied(field.as_bytes()) {
        return Edit::Blank;
    }
    // HALTED(X-COCRDSLC-H1): the evident class-test path, as in `edit_account`.
    if field.as_bytes().iter().all(u8::is_ascii_digit) {
        Edit::Valid(CardNumber::new(field))
    } else {
        Edit::Invalid
    }
}

/// The edits of a request in program order (COCRDSLC-006 to COCRDSLC-013): `2210-EDIT-ACCOUNT`
/// over `acct`, then `2220-EDIT-CARD` over `card_num` unless `card_ref` names the card, then
/// the cross-field edit.
///
/// Each text is read as the terminal sends its field: padded with spaces to 11 or 16 bytes. An
/// input is not supplied when it is absent, empty, all spaces, `*` alone, all low-values (`\0`)
/// or all `0` digits; the account is valid when it is 11 ASCII digits and the card number when
/// it is 16, neither all `0`. A text longer than its field is invalid. A `card_num` that is not
/// supplied counts as absent, so it may accompany a `card_ref`.
///
/// The first message set stands, except that `'No input received'` replaces the account prompt
/// when neither input is supplied (COCRDSLC-008). The failure's field is the account input when
/// the account edit failed and the card input otherwise (COCRDSLC-013).
///
/// `HALTED(X-COCRDSLC-H1)`: which texts that are neither blank nor all digits the COBOL rejects
/// is not fixed by the repository; every such text fails here with the class-test message, the
/// program's evident path, which is no parity result (COCRDSLC-011, COCRDSLC-012).
///
/// # Errors
///
/// A [`DetailFailure`] of class [`FailureClass::Input`] (400), checked in this order:
///
/// - account not supplied: [`NO_SEARCH_CRITERIA_RECEIVED`] when no card is supplied either
///   (COCRDSLC-008), otherwise [`PROMPT_FOR_ACCT`] (COCRDSLC-009, COCRDSLC-007); field
///   `acct_id`;
/// - account neither blank nor valid: [`ACCOUNT_FILTER_INVALID`], field `acct_id`, whatever the
///   card input (COCRDSLC-011);
/// - account valid and no card supplied: [`PROMPT_FOR_CARD`], field `card_num` (COCRDSLC-010);
/// - account valid and a card number neither blank nor valid, or a card number beside a handle:
///   [`CARD_FILTER_INVALID`], field `card_num` (COCRDSLC-012, COCRDSLC-007).
pub fn edit(
    acct: &str,
    card_num: Option<&str>,
    card_ref: Option<CardRef>,
) -> Result<(PicU<11>, CardKey), DetailFailure> {
    let account = edit_account(acct);
    let card = match (card_num.map_or(Edit::Blank, edit_card), card_ref) {
        (Edit::Blank, None) => Edit::Blank,
        (Edit::Blank, Some(handle)) => Edit::Valid(CardKey::ByRef(handle)),
        (Edit::Valid(number), None) => Edit::Valid(CardKey::ByNumber(number)),
        // A typed number beside a handle has no COBOL counterpart: the card input is one field
        // (X-COCRDSLC-5). It fails in the card edit's place, with that edit's own text.
        (Edit::Invalid, _) | (Edit::Valid(_), Some(_)) => Edit::Invalid,
    };
    match (account, card) {
        (Edit::Valid(acct_id), Edit::Valid(key)) => Ok((acct_id, key)),
        (Edit::Blank, Edit::Blank) => Err(DetailFailure::input(
            NO_SEARCH_CRITERIA_RECEIVED,
            DetailField::AcctId,
        )),
        (Edit::Blank, Edit::Valid(_) | Edit::Invalid) => {
            Err(DetailFailure::input(PROMPT_FOR_ACCT, DetailField::AcctId))
        }
        (Edit::Invalid, _) => Err(DetailFailure::input(
            ACCOUNT_FILTER_INVALID,
            DetailField::AcctId,
        )),
        (Edit::Valid(_), Edit::Blank) => {
            Err(DetailFailure::input(PROMPT_FOR_CARD, DetailField::CardNum))
        }
        (Edit::Valid(_), Edit::Invalid) => Err(DetailFailure::input(
            CARD_FILTER_INVALID,
            DetailField::CardNum,
        )),
    }
}

/// The answer to the card read of `9100-GETCARD-BYACCTCARD`, as the service maps the store's
/// answer for the [`CardKey`] [`edit`] returned (COCRDSLC-014 to COCRDSLC-016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardLookup<'a> {
    /// `DFHRESP(NORMAL)`: the card was read.
    Found {
        /// The account [`edit`] returned: the response shows it, never the card's own
        /// `CARD-ACCT-ID`, which the program never compares with it (COCRDSLC-014,
        /// COCRDSLC-018, `X-COCRDSLC-1`).
        acct_id: PicU<11>,
        /// The card's `cards.card_ref` handle, which the response carries so the screen reads
        /// the same card again (COCRDSLC-018, COCRDSLC-026).
        card_ref: CardRef,
        /// The card record read by the key.
        record: &'a CardRecord,
    },
    /// `DFHRESP(NOTFND)`: no card has the number, or no `cards.card_ref` equals the handle
    /// (COCRDSLC-006, COCRDSLC-015).
    NotFound,
    /// Any other answer: the read failed in the store (COCRDSLC-016).
    Error,
}

/// The card number of a card-detail response: the one place where a full number may leave the
/// services (Cardholder Data Redaction, AAP 0.3.4.8). Built only by [`disclose`].
///
/// `Debug` of either variant is the redacted form, so a response that reaches a log line still
/// hides the number; only `Serialize` of [`DisclosedCardNumber`] emits all 16 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardNumberDisclosure {
    /// All 16 characters of `CARD-NUM`, for a verified `usr_type` of `A` (COCRDSLC-022).
    Full(DisclosedCardNumber),
    /// Twelve `*` and the last four digits, for every other user type (COCRDSLC-023, PIC-024,
    /// `E32`).
    Redacted(CardNumber),
}

/// Whether the card-detail response shows `num` in full: [`CardNumberDisclosure::Full`],
/// through [`CardNumber::disclose`], only when `user_type` is `A` (`CDEMO-USRTYP-ADMIN`,
/// COCRDSLC-022); [`CardNumberDisclosure::Redacted`] for `U` and for any other type
/// (COCRDSLC-023).
///
/// `user_type` must be the verified token's `usr_type`, the only input to the choice (AAP
/// 0.5.1): no request field and no COMMAREA copy, which COCRDLIC and this program overwrite
/// with `U` before their transfers, can ask for disclosure. The rule's owner clause never grants
/// it: `CSUSR01Y`, copied at `COCRDSLC.cbl:227` and never referenced, holds a user's ID, names,
/// password and type only (`CSUSR01Y.cpy:17-23`), so no user is linked to a customer, account or
/// card and ownership can never be decided (COCRDSLC-023).
#[must_use]
pub fn disclose(user_type: &UserType, num: &CardNumber) -> CardNumberDisclosure {
    if user_type.is_admin() {
        CardNumberDisclosure::Full(num.disclose())
    } else {
        CardNumberDisclosure::Redacted(num.clone())
    }
}

/// The card-detail response, the fields `1200-SETUP-SCREEN-VARS` moves to the map for a found
/// card (COCRDSLC-018 to COCRDSLC-023). Built by [`view`].
///
/// It carries no CVV: the program moves `CARD-CVV-CD` to no screen field, and no field here can
/// hold one (COCRDSLC-021). No error message accompanies it, as `WS-RETURN-MSG` stays spaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardView {
    /// `ACCTSIDO` from `CC-ACCT-ID`: the account the request names, shown as 11 digits with its
    /// leading zeros (COCRDSLC-018).
    pub acct_id: PicU<11>,
    /// The card's handle, with which the screen reads the same card again (COCRDSLC-018,
    /// COCRDSLC-026).
    pub card_ref: CardRef,
    /// `CARDSIDO` from `CARD-NUM`: full or redacted by [`disclose`] (COCRDSLC-022,
    /// COCRDSLC-023).
    pub card_num: CardNumberDisclosure,
    /// `CRDNAMEO` from `CARD-EMBOSSED-NAME PIC X(50)`, its 50 stored bytes (COCRDSLC-019).
    pub embossed_name: PicX<50>,
    /// `CARD-EXPIRAION-DATE PIC X(10)`, typed; `None` when the stored date is blank
    /// (COCRDSLC-019, CVACT02Y-005, PIC-015). The screen shows its month and year parts
    /// (COCRDSLC-024).
    pub expiration_date: Option<NaiveDate>,
    /// `CRDSTCDO` from `CARD-ACTIVE-STATUS PIC X(01)`, its stored byte (COCRDSLC-019).
    pub status: PicX<1>,
    /// `INFOMSGO`: always [`FOUND_CARDS_FOR_ACCOUNT`], the only information message a response
    /// carries (COCRDSLC-020).
    pub info_message: &'static str,
}

/// `9000-READ-DATA` and `1200-SETUP-SCREEN-VARS`: the response for the outcome of the card read
/// (COCRDSLC-014 to COCRDSLC-023).
///
/// A found card gives a [`CardView`] with the account of the request, the card's handle, its
/// number as [`disclose`] decides it for `user_type`, its embossed name, typed expiry date and
/// status, and [`FOUND_CARDS_FOR_ACCOUNT`]. `user_type` must be the verified token's
/// `usr_type`; nothing else reaches the disclosure (COCRDSLC-022, COCRDSLC-023).
///
/// # Errors
///
/// - [`CardLookup::NotFound`]: [`DID_NOT_FIND_ACCTCARD_COMBO`], class
///   [`FailureClass::NotFound`] (404), field `acct_id` (COCRDSLC-015);
/// - [`CardLookup::Error`]: [`FILE_ERROR`], class [`FailureClass::Store`] (500), field
///   `acct_id`; its `RESP` and `RESP2` fields are `HALTED(H4)` (COCRDSLC-016, COCRDSLC-017).
pub fn view(user_type: &UserType, lookup: CardLookup<'_>) -> Result<CardView, DetailFailure> {
    match lookup {
        CardLookup::Found {
            acct_id,
            card_ref,
            record,
        } => Ok(CardView {
            acct_id,
            card_ref,
            card_num: disclose(user_type, &record.num),
            embossed_name: record.embossed_name.clone(),
            expiration_date: record.expiraion_date.value(),
            status: record.active_status.clone(),
            info_message: FOUND_CARDS_FOR_ACCOUNT,
        }),
        CardLookup::NotFound => Err(DetailFailure {
            message: DID_NOT_FIND_ACCTCARD_COMBO,
            field: DetailField::AcctId,
            class: FailureClass::NotFound,
        }),
        CardLookup::Error => Err(DetailFailure {
            message: FILE_ERROR,
            field: DetailField::AcctId,
            class: FailureClass::Store,
        }),
    }
}

/// The input a [`DetailFailure`] names and the cursor goes to (COCRDSLC-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetailField {
    /// `ACCTSID`, request field `acct_id`: the account edit failed, or the read did not find or
    /// failed to read the card (COCRDSLC-013, COCRDSLC-015, COCRDSLC-016).
    AcctId,
    /// `CARDSID`, request field `card_num`: only the card edit failed (COCRDSLC-013).
    CardNum,
}

impl DetailField {
    /// The request field name `ScreenError.field` carries: `acct_id` or `card_num`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AcctId => "acct_id",
            Self::CardNum => "card_num",
        }
    }
}

/// The status class of a [`DetailFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit failed: 400 (COCRDSLC-007 to COCRDSLC-012).
    Input,
    /// No card has the key: 404 (COCRDSLC-015).
    NotFound,
    /// Reading the card failed: 500 (COCRDSLC-016).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns: 400, 404 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Store => 500,
        }
    }
}

/// An answer to endpoint 8 that is no card: an edit failure, a card not found, or a failed
/// read. The message is the program's literal and holds no input and no card number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DetailFailure {
    message: &'static str,
    field: DetailField,
    class: FailureClass,
}

impl DetailFailure {
    /// An edit failure, class [`FailureClass::Input`].
    const fn input(message: &'static str, field: DetailField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Input,
        }
    }

    /// The message, as the response's `ScreenError` carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the message names and the cursor goes to (COCRDSLC-013).
    #[must_use]
    pub const fn field(&self) -> DetailField {
        self.field
    }

    /// The status class: 400, 404 or 500.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for DetailFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for DetailFailure {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{PicDate, PicU, PicX};
    use proptest::prelude::*;

    use super::{
        ACCOUNT_FILTER_INVALID, CARD_FILTER_INVALID, CardKey, CardLookup, CardNumberDisclosure,
        DID_NOT_FIND_ACCTCARD_COMBO, DetailField, FILE_ERROR, FOUND_CARDS_FOR_ACCOUNT,
        FailureClass, NO_SEARCH_CRITERIA_RECEIVED, PROMPT_FOR_ACCT, PROMPT_FOR_CARD, disclose,
        edit, view,
    };
    use crate::online::cocrdlic::CardRef;
    use crate::online::cosgn00c::UserType;
    use crate::records::card::CardRecord;
    use crate::sensitive::{CardNumber, Cvv};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The raw inputs of one request: `acct_id`, `card_num` and `card_ref`.
    type Request<'a> = (&'a str, Option<&'a str>, Option<CardRef>);

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1111222233334444";

    /// An eleven-digit account with leading zeros.
    const ACCT: &str = "00000000050";

    /// A handle as a COCRDLI list row carries it.
    const HANDLE: CardRef = CardRef::new(7);

    /// A card of account 99, which differs from [`ACCT`], expiring on 2025-04-30.
    fn card(expiry: Option<NaiveDate>) -> Result<CardRecord, Box<dyn StdError>> {
        Ok(CardRecord {
            num: CardNumber::parse(CARD)?,
            acct_id: PicU::new(99)?,
            cvv_cd: Cvv::new(PicU::new(321)?),
            embossed_name: PicX::from_text("Test Holder")?,
            expiraion_date: PicDate::new(expiry)?,
            active_status: PicX::from_text("Y")?,
            filler: PicX::spaces(),
        })
    }

    fn expiry() -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2025, 4, 30).ok_or_else(|| "2025-04-30 is a date".into())
    }

    // audit: COCRDSLC-007
    // audit: COCRDSLC-008
    // audit: COCRDSLC-009
    // audit: COCRDSLC-010
    // audit: COCRDSLC-011
    // audit: COCRDSLC-012
    // audit: COCRDSLC-013
    // audit: COCRDSLC-028
    // audit: COCRDSLC-029
    #[test]
    fn each_edit_failure_has_its_message_field_and_status() {
        let nul = "\0".repeat(11);
        let groups: [(&str, DetailField, Vec<Request<'_>>); 6] = [
            // Neither input supplied: the cross-field text replaces the account prompt.
            (
                NO_SEARCH_CRITERIA_RECEIVED,
                DetailField::AcctId,
                vec![
                    ("", None, None),
                    ("   ", Some(""), None),
                    ("*", Some("*"), None),
                    ("00000000000", Some("0000000000000000"), None),
                    (nul.as_str(), Some("                "), None),
                ],
            ),
            // Account not supplied, the card supplied in any form: the account prompt stands.
            (
                PROMPT_FOR_ACCT,
                DetailField::AcctId,
                vec![
                    ("", Some(CARD), None),
                    ("00000000000", None, Some(HANDLE)),
                    ("*          ", Some("12AB"), None),
                    ("", Some(CARD), Some(HANDLE)),
                ],
            ),
            // Account neither blank nor valid: its class-test text, whatever the card.
            (
                ACCOUNT_FILTER_INVALID,
                DetailField::AcctId,
                vec![
                    ("1234", Some(CARD), None),
                    ("1234567890A", None, None),
                    ("0", Some("x"), None),
                    ("000000000050", None, Some(HANDLE)),
                    (" 0000000050", Some(CARD), Some(HANDLE)),
                    ("-0000000050", Some(CARD), None),
                ],
            ),
            // Account valid, no card supplied.
            (
                PROMPT_FOR_CARD,
                DetailField::CardNum,
                vec![
                    (ACCT, None, None),
                    (ACCT, Some(""), None),
                    (ACCT, Some("*"), None),
                    (ACCT, Some("0000000000000000"), None),
                ],
            ),
            // Account valid, a card number neither blank nor valid.
            (
                CARD_FILTER_INVALID,
                DetailField::CardNum,
                vec![
                    (ACCT, Some("123"), None),
                    (ACCT, Some("111122223333444X"), None),
                    (ACCT, Some("11112222333344445"), None),
                    (ACCT, Some("1111 2222 3333 4"), None),
                ],
            ),
            // Account valid, a card number beside a handle (COCRDSLC-007).
            (
                CARD_FILTER_INVALID,
                DetailField::CardNum,
                vec![
                    (ACCT, Some(CARD), Some(HANDLE)),
                    (ACCT, Some("bad"), Some(HANDLE)),
                    ("12345678901", Some("0000000000000001"), Some(HANDLE)),
                ],
            ),
        ];
        for (message, field, requests) in groups {
            for (acct, card_num, card_ref) in requests {
                let failure = edit(acct, card_num, card_ref).err();
                assert_eq!(failure.map(|f| f.message()), Some(message), "{acct:?}");
                assert_eq!(failure.map(|f| f.field()), Some(field), "{acct:?}");
                assert_eq!(failure.map(|f| f.class()), Some(FailureClass::Input));
                assert_eq!(failure.map(|f| f.class().http_status()), Some(400));
                assert_eq!(failure.map(|f| f.to_string()), Some(message.to_owned()));
            }
        }
    }

    // audit: COCRDSLC-006
    // audit: COCRDSLC-014
    #[test]
    fn valid_requests_yield_the_account_and_the_card_key() -> TestResult {
        let account = PicU::<11>::new(50)?;
        let number = CardNumber::parse(CARD)?;
        assert_eq!(
            edit(ACCT, Some(CARD), None),
            Ok((account, CardKey::ByNumber(number.clone())))
        );
        assert_eq!(
            edit(ACCT, None, Some(HANDLE)),
            Ok((account, CardKey::ByRef(HANDLE)))
        );
        // A card input that is not supplied may accompany the handle.
        assert_eq!(
            edit(ACCT, Some("                "), Some(HANDLE)),
            Ok((account, CardKey::ByRef(HANDLE)))
        );
        assert_eq!(
            edit("99999999999", Some("9999999999999999"), None),
            Ok((
                PicU::new(99_999_999_999)?,
                CardKey::ByNumber(CardNumber::parse("9999999999999999")?)
            ))
        );
        Ok(())
    }

    // audit: COCRDSLC-014
    // audit: COCRDSLC-018
    // audit: COCRDSLC-019
    // audit: COCRDSLC-020
    // audit: COCRDSLC-021
    // audit: COCRDSLC-023
    #[test]
    fn a_found_card_is_shown_redacted_under_the_requested_account() -> TestResult {
        let record = card(Some(expiry()?))?;
        let acct_id = PicU::new(50)?;
        let shown = view(
            &UserType::USER,
            CardLookup::Found {
                acct_id,
                card_ref: HANDLE,
                record: &record,
            },
        )?;
        assert_eq!(shown.acct_id, acct_id);
        assert_ne!(shown.acct_id, record.acct_id);
        assert_eq!(shown.card_ref, HANDLE);
        assert_eq!(
            shown.card_num,
            CardNumberDisclosure::Redacted(CardNumber::parse(CARD)?)
        );
        assert_eq!(shown.embossed_name, record.embossed_name);
        assert_eq!(shown.embossed_name.as_bytes().len(), 50);
        assert_eq!(shown.expiration_date, Some(expiry()?));
        assert_eq!(shown.status, PicX::from_text("Y")?);
        assert_eq!(shown.info_message, "   Displaying requested details");
        assert_eq!(shown.info_message, FOUND_CARDS_FOR_ACCOUNT);
        let rendered = format!("{shown:?}");
        assert!(
            !rendered.contains(CARD),
            "Debug shows the redacted number only"
        );
        assert!(rendered.contains("************4444"));
        Ok(())
    }

    // audit: COCRDSLC-019
    // audit: COCRDSLC-022
    #[test]
    fn an_admin_sees_the_full_number_and_a_blank_expiry_is_none() -> TestResult {
        let record = card(None)?;
        let shown = view(
            &UserType::ADMIN,
            CardLookup::Found {
                acct_id: PicU::new(50)?,
                card_ref: HANDLE,
                record: &record,
            },
        )?;
        assert_eq!(
            shown.card_num,
            CardNumberDisclosure::Full(record.num.disclose())
        );
        assert_eq!(shown.expiration_date, None);
        assert!(!format!("{:?}", shown.card_num).contains(CARD));
        Ok(())
    }

    // audit: COCRDSLC-022
    // audit: COCRDSLC-023
    #[test]
    fn only_type_a_discloses_the_full_number() -> TestResult {
        let number = CardNumber::parse(CARD)?;
        assert_eq!(
            disclose(&UserType::ADMIN, &number),
            CardNumberDisclosure::Full(number.disclose())
        );
        for code in [b'U', b' ', b'a', b'X'] {
            let user_type = UserType::from_code(PicX::from_array([code]));
            let CardNumberDisclosure::Redacted(shown) = disclose(&user_type, &number) else {
                return Err(format!("type {code} must not disclose").into());
            };
            assert_eq!(shown.redacted(), "************4444");
            assert_eq!(shown.to_string(), "************4444");
        }
        Ok(())
    }

    // audit: COCRDSLC-015
    // audit: COCRDSLC-016
    // audit: COCRDSLC-030
    #[test]
    fn a_missing_card_is_404_and_a_failed_read_is_500_on_the_account() {
        let missing = view(&UserType::ADMIN, CardLookup::NotFound).err();
        assert_eq!(
            missing.map(|f| f.message()),
            Some(DID_NOT_FIND_ACCTCARD_COMBO)
        );
        assert_eq!(
            missing.map(|f| f.message()),
            Some("Did not find cards for this search condition")
        );
        assert_eq!(missing.map(|f| f.field()), Some(DetailField::AcctId));
        assert_eq!(missing.map(|f| f.class().http_status()), Some(404));

        let failed = view(&UserType::USER, CardLookup::Error).err();
        assert_eq!(failed.map(|f| f.field()), Some(DetailField::AcctId));
        assert_eq!(failed.map(|f| f.class()), Some(FailureClass::Store));
        assert_eq!(failed.map(|f| f.class().http_status()), Some(500));
        let message = failed.map_or("", |f| f.message());
        assert_eq!(message, FILE_ERROR);
        assert_eq!(message.len(), 75);
        // Bytes 1-48 and 59-65 are asserted; 49-58 and 66-75 are HALTED(H4): BLOCKED.
        assert_eq!(
            message.get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(message.get(58..65), Some(",RESP2 "));
    }

    #[test]
    fn field_names_follow_the_request() {
        assert_eq!(DetailField::AcctId.name(), "acct_id");
        assert_eq!(DetailField::CardNum.name(), "card_num");
        assert_eq!(FailureClass::NotFound.http_status(), 404);
    }

    proptest! {
        // Total Parsers Over Untrusted Input: any text, any handle, never a panic, and a
        // failure is always one of the program's literals.
        #[test]
        fn edit_is_total(
            acct in any::<String>(),
            card_num in proptest::option::of(any::<String>()),
            card_ref in proptest::option::of(any::<i64>()),
        ) {
            if let Err(failure) = edit(&acct, card_num.as_deref(), card_ref.map(CardRef::new)) {
                prop_assert!([
                    NO_SEARCH_CRITERIA_RECEIVED,
                    PROMPT_FOR_ACCT,
                    PROMPT_FOR_CARD,
                    ACCOUNT_FILTER_INVALID,
                    CARD_FILTER_INVALID,
                ]
                .contains(&failure.message()));
            }
        }
    }
}
