//! The wire types of the card transactions: `CCLI` (`COCRDLIC`, map `COCRDLI`,
//! `GET /cards`), `CCDL` (`COCRDSLC`, map `COCRDSL`, `GET /cards/{id}`) and
//! `CCUP` (`COCRDUPC`, map `COCRDUP`, `PUT /cards/{id}`).
//!
//! Requests carry the map fields as entered and bound only their lengths, the
//! widths of the `COCRDLI`, `COCRDSL` and `COCRDUP` input fields
//! [app/cpy-bms/COCRDLI.CPY] [app/cpy-bms/COCRDSL.CPY]
//! [app/cpy-bms/COCRDUP.CPY]: `ACCTSID X(11)`, `CARDSID X(16)`,
//! `CRDNAME X(50)`, `CRDSTCD X(1)`, `EXPMON X(2)` and `EXPYEAR X(4)`. Every
//! other edit is the rule's, so a blank, zero or non-numeric value is answered
//! with the audited COBOL text of `docs/parity/validation-audit.md`.
//!
//! A value longer than its map field has no 3270 counterpart: the field holds
//! no more characters. The validation audit records no row and no message for
//! it, so it is rejected with 400, kind `Invalid`, an empty `message` and the
//! BMS fields of the overlong values, in the audit's `Order`.
//!
//! Responses carry the `CVACT02Y` `CARD-RECORD` fields the maps show, under the
//! copybook names (`card_expiraion_date`, spelling kept) and the request-fixed
//! keys `card_number` and `account_id`. The verification code `CARD-CVV-CD` is
//! never shown by `COCRDSL` or `COCRDUP` [app/cbl/COCRDSLC.cbl:L463-484]
//! [app/cbl/COCRDUPC.cbl:L1088-1117] and has no field in any type here.
//!
//! `Debug` shows card numbers masked to their last four characters, as
//! [`CardNumber`] masks them, so a logged request or response holds no full
//! card number.

use std::fmt;

use carddemo_domain::entities::Card;
use carddemo_domain::numeric::CardNumber;
use carddemo_domain::ports::Direction;
use carddemo_domain::rules::{VersionToken, card_list, card_update, card_view};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

use crate::dto::{BmsField, id_string, reject_invalid, text};
use crate::error::ApiError;

/// The `message` of an overlong value: empty, because the validation audit
/// records no over-length row for `COCRDLIC`, `COCRDSLC` or `COCRDUPC` and no
/// COBOL text exists for an input the map field cannot hold.
const OVER_LENGTH_MESSAGE: &str = "";

/// `CARDSIDI PIC X(16)` [app/cpy-bms/COCRDUP.CPY:L66], the bound of the
/// `{id}` path segment of `PUT /cards/{id}`.
const CARD_NUMBER_LEN: usize = 16;

/// The name the `{id}` path segment of `PUT /cards/{id}` takes among the
/// length errors of [`CardUpdateRequest::check`].
const PATH_CARD_NUMBER: &str = "card_number";

/// The `validator` code of a failed length check.
const LENGTH_CODE: &str = "length";

/// A card number as `Debug` shows it: masked through [`CardNumber`].
fn masked(number: &str) -> CardNumber {
    CardNumber::move_from(number)
}

/// A `<Dto>Field` enum: one variant per BMS field, its serde name and
/// `as_str` the field name, and its rank the lowest `Order` of the field's
/// rows in `docs/parity/validation-audit.md`.
///
/// The name is written once and serves both the serde rename and `as_str`, so
/// the two cannot differ.
macro_rules! bms_field_enum {
    (
        $(#[$enum_attr:meta])*
        $name:ident {
            $(
                $(#[$variant_attr:meta])*
                // `tt`, not `literal`: a `literal` fragment reaches the derives
                // wrapped in an invisible group, which utoipa's reading of
                // `#[serde(rename)]` does not see through.
                $variant:ident = $bms:tt, order $order:tt;
            )+
        }
    ) => {
        $(#[$enum_attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        pub enum $name {
            $(
                $(#[$variant_attr])*
                #[serde(rename = $bms)]
                $variant,
            )+
        }

        impl $name {
            /// The BMS symbolic field name without its suffix: the
            /// `fields[].field` value of the error body.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $bms,)+
                }
            }
        }

        impl BmsField for $name {
            fn as_str(self) -> &'static str {
                Self::as_str(self)
            }

            fn rank(self) -> u16 {
                match self {
                    $(Self::$variant => $order,)+
                }
            }
        }
    };
}

bms_field_enum! {
    /// The BMS fields a `GET /cards` error names: the `BMS field` values of
    /// the `COCRDLIC` rows. The selector rows V-COCRDLIC-003 and
    /// V-COCRDLIC-004 name none.
    CardListField {
        /// `ACCTSID`, the account filter — V-COCRDLIC-001, V-COCRDLIC-005 to
        /// V-COCRDLIC-010.
        AcctSid = "ACCTSID", order 1;
        /// `CARDSID`, the card filter — V-COCRDLIC-002.
        CardSid = "CARDSID", order 2;
    }
}

bms_field_enum! {
    /// The BMS fields a `GET /cards/{id}` error names: the `BMS field` values
    /// of the `COCRDSLC` rows.
    CardViewField {
        /// `ACCTSID`, the account number — V-COCRDSLC-001, V-COCRDSLC-002,
        /// V-COCRDSLC-005 to V-COCRDSLC-007.
        AcctSid = "ACCTSID", order 1;
        /// `CARDSID`, the card number — V-COCRDSLC-003, V-COCRDSLC-004.
        CardSid = "CARDSID", order 3;
    }
}

bms_field_enum! {
    /// The BMS fields a `PUT /cards/{id}` error names: the `BMS field` values
    /// of the `COCRDUPC` rows. V-COCRDUPC-001, the missing version token,
    /// names none.
    CardUpdateField {
        /// `ACCTSID`, the account number — V-COCRDUPC-002, V-COCRDUPC-003,
        /// V-COCRDUPC-006 to V-COCRDUPC-008, V-COCRDUPC-018, V-COCRDUPC-020.
        AcctSid = "ACCTSID", order 2;
        /// `CARDSID`, the card number of the path — V-COCRDUPC-004,
        /// V-COCRDUPC-005.
        CardSid = "CARDSID", order 4;
        /// `CRDNAME`, the embossed name — V-COCRDUPC-009 to V-COCRDUPC-011,
        /// V-COCRDUPC-019.
        CrdName = "CRDNAME", order 8;
        /// `CRDSTCD`, the active status — V-COCRDUPC-012, V-COCRDUPC-013.
        CrdStCd = "CRDSTCD", order 11;
        /// `EXPMON`, the expiry month — V-COCRDUPC-014, V-COCRDUPC-015.
        ExpMon = "EXPMON", order 13;
        /// `EXPYEAR`, the expiry year — V-COCRDUPC-016, V-COCRDUPC-017.
        ExpYear = "EXPYEAR", order 15;
    }
}

/// The BMS field of a `GET /cards` query parameter `validator` reports.
fn list_field(name: &str) -> Option<CardListField> {
    match name {
        "account_id" => Some(CardListField::AcctSid),
        "card_number" => Some(CardListField::CardSid),
        _ => None,
    }
}

/// The BMS field of a `GET /cards/{id}` query parameter `validator` reports.
fn view_field(name: &str) -> Option<CardViewField> {
    match name {
        "account_id" => Some(CardViewField::AcctSid),
        _ => None,
    }
}

/// The BMS field of a `PUT /cards/{id}` value `validator` reports: a body
/// field, or [`PATH_CARD_NUMBER`] for the `{id}` segment.
fn update_field(name: &str) -> Option<CardUpdateField> {
    match name {
        "account_id" => Some(CardUpdateField::AcctSid),
        PATH_CARD_NUMBER => Some(CardUpdateField::CardSid),
        "embossed_name" => Some(CardUpdateField::CrdName),
        "active_status" => Some(CardUpdateField::CrdStCd),
        "expiry_month" => Some(CardUpdateField::ExpMon),
        "expiry_year" => Some(CardUpdateField::ExpYear),
        _ => None,
    }
}

/// The filters of `GET /cards`, the `COCRDLI` map's `ACCTSID` and `CARDSID`
/// input fields as entered.
///
/// The page parameters are the separate
/// [`PageQuery`](crate::pagination::PageQuery) extractor of the same query
/// string, never `#[serde(flatten)]`ed into this struct: `serde_urlencoded`
/// cannot deserialize a number through `flatten`, so the handler takes the
/// filters and the page as two extractors and declares both in `params(..)`.
#[derive(Clone, Default, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct CardListQuery {
    /// The account filter, `ACCTSIDI X(11)`: 11 digits, or blank or all zeros
    /// for no filter (`2210-EDIT-ACCOUNT`).
    #[serde(default)]
    #[param(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
    /// The card filter, `CARDSIDI X(16)`: 16 digits, or blank or all zeros
    /// for no filter (`2220-EDIT-CARD`).
    #[serde(default)]
    #[param(max_length = 16)]
    #[validate(length(max = 16))]
    pub card_number: Option<String>,
}

impl fmt::Debug for CardListQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CardListQuery")
            .field("account_id", &self.account_id)
            .field("card_number", &self.card_number.as_deref().map(masked))
            .finish()
    }
}

impl CardListQuery {
    /// COCRDLIC 2210-EDIT-ACCOUNT, 2220-EDIT-CARD — V-COCRDLIC-001,
    /// V-COCRDLIC-002 (field lengths only)
    ///
    /// Rejects a filter longer than its map field before the rule's edits,
    /// which answer every other filter.
    ///
    /// # Errors
    ///
    /// 400, kind `Invalid`, with an empty `message` and `ACCTSID` or
    /// `CARDSID` (or both, in that order) when `account_id` holds more than 11
    /// characters or `card_number` more than 16.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, list_field, OVER_LENGTH_MESSAGE))
    }

    /// COCRDLIC 0000-MAIN — P-COCRDLIC-003, P-COCRDLIC-014
    ///
    /// The rule input of one `GET /cards` request: the filters as entered and
    /// the page that [`PageQuery::resolve`](crate::pagination::PageQuery::resolve)
    /// gives with [`card_list::SCREEN_ROWS`] as its default.
    ///
    /// `start_key` becomes the browse key as the COBOL `MOVE` into the `X(16)`
    /// first- and last-card keys of the COMMAREA makes it: padded with spaces
    /// and cut after 16 characters. The keys a page returns are card numbers,
    /// so a key passed back unchanged is never cut.
    #[must_use]
    pub fn into_input(
        self,
        start_key: Option<String>,
        direction: Direction,
        page_size: u16,
    ) -> card_list::CardListInput {
        card_list::CardListInput {
            account_filter: self.account_id,
            card_filter: self.card_number,
            start_key: start_key.map(|key| CardNumber::move_from(&key)),
            direction,
            page_size,
        }
    }
}

/// One row of the card list: the `ACCTNOn`, `CRDNUMn` and `CRDSTSn` fields
/// of the `COCRDLI` map, which `1200-SCREEN-ARRAY-INIT` fills from the
/// `CARDDAT` record [app/cbl/COCRDLIC.cbl:L684-686] — P-COCRDLIC-017.
///
/// The row selector `CRDSELn` has no field: the list's row actions open the
/// card view and edit routes instead (E-54).
#[derive(Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardSummary {
    /// `CARD-NUM X(16)` (`CRDNUMn`), right-trimmed.
    pub card_number: String,
    /// `CARD-ACCT-ID 9(11)` (`ACCTNOn`), as its 11 zero-padded digits.
    pub account_id: String,
    /// `CARD-ACTIVE-STATUS X(01)` (`CRDSTSn`), `Y` or `N` in the fixtures.
    pub card_active_status: String,
}

impl fmt::Debug for CardSummary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CardSummary")
            .field("card_number", &masked(&self.card_number))
            .field("account_id", &self.account_id)
            .field("card_active_status", &self.card_active_status)
            .finish()
    }
}

impl From<&Card> for CardSummary {
    fn from(card: &Card) -> Self {
        Self {
            card_number: id_string(&card.card_num),
            account_id: id_string(&card.card_acct_id),
            card_active_status: text(&card.card_active_status),
        }
    }
}

/// The account key of `GET /cards/{id}`: the `COCRDSL` map's `ACCTSID` input
/// field as entered (E-55). The card number is the `{id}` path segment.
#[derive(Debug, Clone, Default, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct CardViewQuery {
    /// The account number, `ACCTSIDI X(11)`: 11 digits. Absent is a blank
    /// account, which `2210-EDIT-ACCOUNT` answers 'Account number not
    /// provided'. The card is read by its number alone, whatever account is
    /// entered (E-55).
    #[serde(default)]
    #[param(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
}

impl CardViewQuery {
    /// COCRDSLC 2210-EDIT-ACCOUNT — V-COCRDSLC-001, V-COCRDSLC-002 (field
    /// length only)
    ///
    /// Rejects an account number longer than its map field before the rule's
    /// edits, which answer every other value. The card number of the path is
    /// edited by the rule, whose `2220-EDIT-CARD` answers a longer one with
    /// its not-numeric text (V-COCRDSLC-004).
    ///
    /// # Errors
    ///
    /// 400, kind `Invalid`, with an empty `message` and `ACCTSID` when
    /// `account_id` holds more than 11 characters.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, view_field, OVER_LENGTH_MESSAGE))
    }

    /// COCRDSLC 2100-RECEIVE-MAP — P-COCRDSLC-014
    ///
    /// The rule input of one `GET /cards/{id}` request: the account number as
    /// entered and `card_number`, the `{id}` path segment, unchanged.
    #[must_use]
    pub fn into_input(self, card_number: String) -> card_view::CardViewInput {
        card_view::CardViewInput {
            account_id: self.account_id,
            card_number: Some(card_number),
        }
    }
}

/// A card as `COCRDSL` shows it and `COCRDUP` fetches it: the two keys, the
/// embossed name, the expiry date and the active status
/// [app/cbl/COCRDSLC.cbl:L463-484] [app/cbl/COCRDUPC.cbl:L1088-1117], with
/// the token `PUT /cards/{id}` echoes back.
///
/// The maps show the expiry as `EXPMON` and `EXPYEAR` (and `EXPDAY`,
/// protected, on `COCRDUP`), cut from the `X(10)` date; the response keeps the
/// stored text and the SPA cuts it. `CARD-CVV-CD` is not part of the screen
/// and is never serialized.
#[derive(Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardResponse {
    /// `CARD-NUM X(16)` (`CARDSID`), right-trimmed.
    pub card_number: String,
    /// `CARD-ACCT-ID 9(11)` (`ACCTSID`), as its 11 zero-padded digits: the
    /// card's own account, which may differ from the account entered (E-55).
    pub account_id: String,
    /// `CARD-EMBOSSED-NAME X(50)` (`CRDNAME`), right-trimmed, in its stored
    /// case.
    pub card_embossed_name: String,
    /// `CARD-EXPIRAION-DATE X(10)`, right-trimmed: `YYYY-MM-DD` in the
    /// fixtures, whose year `(1:4)`, month `(6:2)` and day `(9:2)` are
    /// `EXPYEAR`, `EXPMON` and `EXPDAY`.
    pub card_expiraion_date: String,
    /// `CARD-ACTIVE-STATUS X(01)` (`CRDSTCD`).
    pub card_active_status: String,
    /// The card's version token, 16 lower-case hex digits: the keyed digest of
    /// the fields `9300-CHECK-CHANGE-IN-REC` compares (AAP 0.3.7). A
    /// `PUT /cards/{id}` sends it back unchanged.
    #[schema(pattern = "^[0-9a-f]{16}$")]
    pub version: String,
}

impl fmt::Debug for CardResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CardResponse")
            .field("card_number", &masked(&self.card_number))
            .field("account_id", &self.account_id)
            .field("card_embossed_name", &self.card_embossed_name)
            .field("card_expiraion_date", &self.card_expiraion_date)
            .field("card_active_status", &self.card_active_status)
            .field("version", &self.version)
            .finish()
    }
}

impl CardResponse {
    /// COCRDSLC 1200-SETUP-SCREEN-VARS — P-COCRDSLC-006
    ///
    /// The card a successful `GET /cards/{id}` shows, with its token.
    #[must_use]
    pub fn from_view(view: &card_view::CardView) -> Self {
        Self::from_card(&view.card, &view.version)
    }

    /// COCRDUPC 3200-SETUP-SCREEN-VARS — P-COCRDUPC-019
    ///
    /// `card` with `version`, its token: the card a successful
    /// `PUT /cards/{id}` leaves, with the token of the record as it now
    /// stands, which `card_update::record_version` computes for it.
    #[must_use]
    pub fn from_card(card: &Card, version: &VersionToken) -> Self {
        Self {
            card_number: id_string(&card.card_num),
            account_id: id_string(&card.card_acct_id),
            card_embossed_name: text(&card.card_embossed_name),
            card_expiraion_date: text(&card.card_expiraion_date),
            card_active_status: text(&card.card_active_status),
            version: version.as_str().to_owned(),
        }
    }
}

/// The body of `PUT /cards/{id}`: the `COCRDUP` map's input fields as
/// entered. The card number is the `{id}` path segment.
///
/// Each field reaches the rule unchanged, an absent one as empty text, which
/// `1100-RECEIVE-MAP` reads as a field not entered. `version` is parsed by the
/// handler, `crate::handlers::cards`: a missing or malformed token is the
/// no-message 400 of V-COCRDUPC-001 (E-24), answered before any COBOL edit.
#[derive(Debug, Clone, Default, Deserialize, ToSchema, Validate)]
pub struct CardUpdateRequest {
    /// The account number, `ACCTSIDI X(11)`: 11 digits (E-55).
    #[serde(default)]
    #[schema(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
    /// The embossed name, `CRDNAMEI X(50)`: letters and spaces.
    #[serde(default)]
    #[schema(max_length = 50)]
    #[validate(length(max = 50))]
    pub embossed_name: Option<String>,
    /// The active status, `CRDSTCDI X(1)`: `Y` or `N`.
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub active_status: Option<String>,
    /// The expiry month, `EXPMONI X(2)`: `01` to `12`.
    #[serde(default)]
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub expiry_month: Option<String>,
    /// The expiry year, `EXPYEARI X(4)`: `1950` to `2099`.
    #[serde(default)]
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub expiry_year: Option<String>,
    /// The `version` of the card as `GET /cards/{id}` returned it: 16
    /// lower-case hex digits.
    // No length rule: the handler parses the token with `VersionToken::parse`
    // and answers anything else with the no-message 400 of E-24; the pattern
    // publishes the format that parse accepts.
    #[serde(default)]
    #[schema(pattern = "^[0-9a-f]{16}$")]
    pub version: Option<String>,
}

impl CardUpdateRequest {
    /// COCRDUPC 1100-RECEIVE-MAP — V-COCRDUPC-002 to V-COCRDUPC-005,
    /// V-COCRDUPC-010 to V-COCRDUPC-017 (field lengths only)
    ///
    /// Rejects a body field, or `card_number` (the `{id}` path segment), longer
    /// than its map field before the rule's edits, which answer every other
    /// value.
    ///
    /// The handler passes the `{id}` segment as `card_number`. The path is
    /// checked here, with the body, because the rule moves each value into
    /// its `X(n)` field as the map receives it, cut at the field's width
    /// (P-COCRDUPC-010): a longer card number would otherwise update the card
    /// its first 16 characters name.
    ///
    /// # Errors
    ///
    /// 400, kind `Invalid`, with an empty `message` and the BMS fields of the
    /// overlong values in the audit's order: `ACCTSID`, `CARDSID`, `CRDNAME`,
    /// `CRDSTCD`, `EXPMON`, `EXPYEAR`.
    pub fn check(&self, card_number: &str) -> Result<(), ApiError> {
        let mut errors = self.validate().err().unwrap_or_default();
        // `validator` counts characters, and so does this bound.
        if card_number.chars().count() > CARD_NUMBER_LEN {
            errors.add(PATH_CARD_NUMBER, ValidationError::new(LENGTH_CODE));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(reject_invalid(&errors, update_field, OVER_LENGTH_MESSAGE))
        }
    }

    /// COCRDUPC 1100-RECEIVE-MAP — P-COCRDUPC-010
    ///
    /// The rule input of one `PUT /cards/{id}` request: the body fields as
    /// entered, an absent one as empty text, `card_number`, the `{id}` path
    /// segment, unchanged, and `version`, the token the handler parsed from
    /// [`CardUpdateRequest::version`].
    #[must_use]
    pub fn into_input(
        self,
        card_number: String,
        version: VersionToken,
    ) -> card_update::CardUpdateInput {
        card_update::CardUpdateInput {
            account_id: self.account_id.unwrap_or_default(),
            card_number,
            embossed_name: self.embossed_name.unwrap_or_default(),
            active_status: self.active_status.unwrap_or_default(),
            expiry_month: self.expiry_month.unwrap_or_default(),
            expiry_year: self.expiry_year.unwrap_or_default(),
            version,
        }
    }
}

/// The answer to a successful `PUT /cards/{id}`: the card as it now stands
/// and the information text `COCRDUPC` shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardUpdateResponse {
    /// The card rewritten, or the card fetched when nothing changed, with its
    /// current token.
    pub card: CardResponse,
    /// 'Changes committed to database' after a rewrite (P-COCRDUPC-007), or
    /// 'No change detected with respect to values fetched.' when nothing
    /// changed (V-COCRDUPC-009).
    pub message: String,
}

impl CardUpdateResponse {
    /// COCRDUPC 3250-SETUP-INFOMSG — P-COCRDUPC-007, V-COCRDUPC-009
    ///
    /// The response for `updated`, the outcome of the rule, whose card has
    /// the token `version`.
    #[must_use]
    pub fn from_updated(updated: &card_update::CardUpdated, version: &VersionToken) -> Self {
        Self {
            card: CardResponse::from_card(&updated.card, version),
            message: updated.message.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fmt::Debug;

    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::Card;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, Cvv, NumericError};
    use carddemo_domain::ports::Direction;
    use carddemo_domain::rules::VersionToken;
    use carddemo_domain::rules::card_list::CardListInput;
    use carddemo_domain::rules::card_update::{CardUpdateInput, CardUpdated};
    use carddemo_domain::rules::card_view::{CardView, CardViewInput};
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema};

    use super::{
        CardListField, CardListQuery, CardResponse, CardSummary, CardUpdateField,
        CardUpdateRequest, CardUpdateResponse, CardViewField, CardViewQuery,
    };
    use crate::dto::BmsField;
    use crate::error::ApiError;

    /// The first fixture card, of account 00000000050 (P-CVACT02Y-009)
    /// [app/data/ASCII/carddata.txt:L1].
    const CARD: &str = "0500024453765740";

    /// The second fixture card (P-CVACT02Y-012) [app/data/ASCII/carddata.txt:L2].
    const OTHER_CARD: &str = "0683586198171516";

    /// A well-formed version token.
    const TOKEN: &str = "0123456789abcdef";

    /// The verification code of [`fixture_card`], which no response may show.
    const CVV: u16 = 747;

    fn fixture_card() -> Result<Card, NumericError> {
        Ok(Card {
            card_num: CardNumber::move_from(CARD),
            card_acct_id: AccountId::new(50)?,
            card_cvv_cd: Cvv::new(CVV)?,
            card_embossed_name: Alnum::move_from("Aniya Von"),
            card_expiraion_date: Alnum::move_from("2023-03-09"),
            card_active_status: Alnum::move_from("Y"),
        })
    }

    fn token() -> VersionToken {
        VersionToken::parse(TOKEN).expect("a well-formed token")
    }

    /// `n` digits: one character per digit.
    fn digits(n: usize) -> String {
        "9".repeat(n)
    }

    #[test]
    fn card_summary_trims_text_and_zero_pads_the_account() -> Result<(), NumericError> {
        // (card number, account, status) -> (card_number, account_id, status)
        let cases = [
            ((CARD, 50, "Y"), (CARD, "00000000050", "Y")),
            (("4111", 99_999_999_999, "N"), ("4111", "99999999999", "N")),
            ((OTHER_CARD, 1, " "), (OTHER_CARD, "00000000001", "")),
        ];
        for ((number, account, status), (card_number, account_id, active)) in cases {
            let card = Card {
                card_num: CardNumber::move_from(number),
                card_acct_id: AccountId::new(account)?,
                card_active_status: Alnum::move_from(status),
                ..fixture_card()?
            };
            let summary = CardSummary::from(&card);
            assert_eq!(summary.card_number, card_number, "{number}");
            assert_eq!(summary.account_id, account_id, "{number}");
            assert_eq!(summary.card_active_status, active, "{number}");
        }
        Ok(())
    }

    #[test]
    fn card_response_carries_the_shown_fields_and_never_the_cvv() -> Result<(), NumericError> {
        let card = fixture_card()?;
        let response = CardResponse::from_card(&card, &token());
        let json = serde_json::to_value(&response).expect("serializes");
        assert_eq!(
            json,
            json!({
                "card_number": CARD,
                "account_id": "00000000050",
                "card_embossed_name": "Aniya Von",
                "card_expiraion_date": "2023-03-09",
                "card_active_status": "Y",
                "version": TOKEN,
            })
        );
        let text = json.to_string();
        assert!(!text.contains("cvv"), "{text}");
        assert!(!text.contains(&CVV.to_string()), "{text}");

        let view = CardView {
            card: card.clone(),
            version: token(),
            message: messages::DISPLAYING_REQUESTED_DETAILS,
        };
        assert_eq!(CardResponse::from_view(&view), response);
        Ok(())
    }

    #[test]
    fn update_response_carries_the_card_and_the_rule_text() -> Result<(), NumericError> {
        for message in [
            messages::CHANGES_COMMITTED_TO_DATABASE,
            messages::NO_CHANGE_DETECTED_WITH_RESPECT_TO_VALUES_FETCHED,
        ] {
            let updated = CardUpdated {
                card: fixture_card()?,
                message,
            };
            let response = CardUpdateResponse::from_updated(&updated, &token());
            assert_eq!(
                response.card,
                CardResponse::from_card(&updated.card, &token())
            );
            let json = serde_json::to_value(&response).expect("serializes");
            assert_eq!(json["message"], message);
            assert_eq!(json["card"]["version"], TOKEN);
            assert!(json["card"].get("card_cvv_cd").is_none());
        }
        Ok(())
    }

    #[test]
    fn debug_masks_every_card_number() -> Result<(), NumericError> {
        let card = fixture_card()?;
        let query = CardListQuery {
            account_id: None,
            card_number: Some(CARD.to_owned()),
        };
        let shown = [
            format!("{query:?}"),
            format!("{:?}", CardSummary::from(&card)),
            format!("{:?}", CardResponse::from_card(&card, &token())),
            format!(
                "{:?}",
                CardUpdateResponse::from_updated(
                    &CardUpdated {
                        card,
                        message: messages::CHANGES_COMMITTED_TO_DATABASE,
                    },
                    &token(),
                )
            ),
        ];
        for text in shown {
            assert!(!text.contains(CARD), "{text}");
            assert!(text.contains("************5740"), "{text}");
        }
        Ok(())
    }

    /// The `fields[].field` values of a rejected check, after asserting the
    /// rest of the 400 body.
    fn rejected_fields(outcome: Result<(), ApiError>) -> Vec<String> {
        let error = outcome.expect_err("the check rejects the request");
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        let body = error.body();
        assert_eq!(body.code, ErrorKind::Invalid.code());
        assert_eq!(body.message, "");
        assert!(body.fields.iter().all(|field| field.message.is_empty()));
        body.fields
            .iter()
            .map(|field| field.field.clone())
            .collect()
    }

    /// Every field of an update body one character over its map field.
    fn overlong_update() -> CardUpdateRequest {
        CardUpdateRequest {
            account_id: Some(digits(12)),
            embossed_name: Some("A".repeat(51)),
            active_status: Some("YY".to_owned()),
            expiry_month: Some(digits(3)),
            expiry_year: Some(digits(5)),
            version: Some(TOKEN.to_owned()),
        }
    }

    #[test]
    fn overlong_values_are_rejected_at_their_bms_fields_in_audit_order() {
        let over_card = "9".repeat(17);
        let cases: Vec<(Result<(), ApiError>, Vec<&str>)> = vec![
            (
                CardListQuery {
                    account_id: None,
                    card_number: Some(over_card.clone()),
                }
                .check(),
                vec!["CARDSID"],
            ),
            (
                CardListQuery {
                    account_id: Some(digits(12)),
                    card_number: None,
                }
                .check(),
                vec!["ACCTSID"],
            ),
            (
                CardListQuery {
                    account_id: Some(digits(12)),
                    card_number: Some(over_card.clone()),
                }
                .check(),
                vec!["ACCTSID", "CARDSID"],
            ),
            (
                CardViewQuery {
                    account_id: Some(digits(12)),
                }
                .check(),
                vec!["ACCTSID"],
            ),
            (
                CardUpdateRequest {
                    embossed_name: Some("A".repeat(51)),
                    ..CardUpdateRequest::default()
                }
                .check(CARD),
                vec!["CRDNAME"],
            ),
            (
                CardUpdateRequest::default().check(&over_card),
                vec!["CARDSID"],
            ),
            (
                overlong_update().check(&over_card),
                vec![
                    "ACCTSID", "CARDSID", "CRDNAME", "CRDSTCD", "EXPMON", "EXPYEAR",
                ],
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(rejected_fields(outcome), expected);
        }
    }

    #[test]
    fn values_at_their_limits_blank_or_absent_pass() {
        let at_limit = CardUpdateRequest {
            account_id: Some(digits(11)),
            embossed_name: Some("A".repeat(50)),
            active_status: Some("Y".to_owned()),
            expiry_month: Some(digits(2)),
            expiry_year: Some(digits(4)),
            // The token is the handler's to edit: any text passes here.
            version: Some("not a token".to_owned()),
        };
        let blank = CardUpdateRequest {
            account_id: Some(String::new()),
            embossed_name: Some(" ".repeat(50)),
            active_status: Some(String::new()),
            expiry_month: Some("  ".to_owned()),
            expiry_year: Some(String::new()),
            version: None,
        };
        let outcomes = [
            CardListQuery {
                account_id: Some(digits(11)),
                card_number: Some(digits(16)),
            }
            .check(),
            CardListQuery {
                account_id: Some(" ".repeat(11)),
                card_number: Some(String::new()),
            }
            .check(),
            CardListQuery::default().check(),
            CardViewQuery {
                account_id: Some(digits(11)),
            }
            .check(),
            CardViewQuery::default().check(),
            at_limit.check(CARD),
            blank.check(""),
            CardUpdateRequest::default().check(&"\u{e9}".repeat(16)),
        ];
        for (index, outcome) in outcomes.into_iter().enumerate() {
            assert!(outcome.is_ok(), "case {index}: {outcome:?}");
        }
    }

    #[test]
    fn into_input_passes_every_value_as_entered() {
        // (start key, direction, rows) -> expected start key
        let cases = [
            (Some(OTHER_CARD), Direction::Backward, 7, Some(OTHER_CARD)),
            (None, Direction::Forward, 100, None),
            (Some(""), Direction::Forward, 1, Some("")),
        ];
        for (start, direction, rows, expected) in cases {
            let query = CardListQuery {
                account_id: Some(" 50".to_owned()),
                card_number: None,
            };
            let input = query.into_input(start.map(str::to_owned), direction, rows);
            assert_eq!(
                input,
                CardListInput {
                    account_filter: Some(" 50".to_owned()),
                    card_filter: None,
                    start_key: expected.map(CardNumber::move_from),
                    direction,
                    page_size: rows,
                }
            );
        }

        let view = CardViewQuery {
            account_id: Some("0000000005A".to_owned()),
        }
        .into_input(CARD.to_owned());
        assert_eq!(
            view,
            CardViewInput {
                account_id: Some("0000000005A".to_owned()),
                card_number: Some(CARD.to_owned()),
            }
        );
        assert_eq!(
            CardViewQuery::default()
                .into_input(" ".to_owned())
                .account_id,
            None
        );

        let update = CardUpdateRequest {
            account_id: Some("00000000050".to_owned()),
            embossed_name: Some("aniya von ".to_owned()),
            active_status: Some("y".to_owned()),
            expiry_month: Some("3".to_owned()),
            expiry_year: None,
            version: Some("ignored here".to_owned()),
        }
        .into_input(CARD.to_owned(), token());
        assert_eq!(
            update,
            CardUpdateInput {
                account_id: "00000000050".to_owned(),
                card_number: CARD.to_owned(),
                embossed_name: "aniya von ".to_owned(),
                active_status: "y".to_owned(),
                expiry_month: "3".to_owned(),
                expiry_year: String::new(),
                version: token(),
            }
        );
    }

    fn list_query(text: &str) -> CardListQuery {
        let uri: Uri = text.parse().expect("a valid URI");
        let Query(query) = Query::<CardListQuery>::try_from_uri(&uri).expect("deserializes");
        query
    }

    #[test]
    fn queries_and_body_deserialize_absent_fields_as_none() {
        // (query string, account_id, card_number)
        let cases = [
            ("/cards", None, None),
            (
                "/cards?account_id=00000000050&card_number=",
                Some("00000000050"),
                Some(""),
            ),
            // The page parameters belong to `PageQuery` and are ignored here.
            (
                "/cards?card_number=0500024453765740&start_key=x&direction=backward&page_size=7",
                None,
                Some(CARD),
            ),
        ];
        for (text, account_id, card_number) in cases {
            let query = list_query(text);
            assert_eq!(query.account_id.as_deref(), account_id, "{text}");
            assert_eq!(query.card_number.as_deref(), card_number, "{text}");
        }

        let uri: Uri = "/cards/0500024453765740?account_id=%2000000000050"
            .parse()
            .expect("a valid URI");
        let Query(view) = Query::<CardViewQuery>::try_from_uri(&uri).expect("deserializes");
        assert_eq!(view.account_id.as_deref(), Some(" 00000000050"));

        let body: CardUpdateRequest =
            serde_json::from_value(json!({"account_id": "00000000050", "version": TOKEN}))
                .expect("deserializes");
        assert_eq!(body.account_id.as_deref(), Some("00000000050"));
        assert_eq!(body.version.as_deref(), Some(TOKEN));
        assert_eq!(body.embossed_name, None);
        let empty: CardUpdateRequest = serde_json::from_value(json!({})).expect("deserializes");
        assert_eq!(empty.expiry_year, None);
    }

    /// The `maxLength` of each property or parameter `items` declares, by
    /// name.
    fn max_lengths<'a>(
        items: impl IntoIterator<Item = (&'a str, &'a Value)>,
    ) -> Vec<(String, u64)> {
        items
            .into_iter()
            .filter_map(|(name, schema)| {
                schema["maxLength"]
                    .as_u64()
                    .map(|max| (name.to_owned(), max))
            })
            .collect()
    }

    fn query_parameters<P: IntoParams>() -> Value {
        serde_json::to_value(P::into_params(|| None)).expect("serializes")
    }

    #[test]
    fn query_parameters_declare_their_map_field_lengths() {
        // (parameters, expected (name, maxLength))
        let cases = [
            (
                query_parameters::<CardListQuery>(),
                vec![("account_id", 11), ("card_number", 16)],
            ),
            (
                query_parameters::<CardViewQuery>(),
                vec![("account_id", 11)],
            ),
        ];
        for (parameters, expected) in cases {
            let parameters = parameters.as_array().expect("a parameter list").clone();
            for parameter in &parameters {
                assert_eq!(parameter["in"], "query", "{parameter}");
                assert_ne!(parameter["required"], true, "{parameter}");
            }
            let declared = max_lengths(parameters.iter().map(|parameter| {
                (
                    parameter["name"].as_str().expect("a name"),
                    &parameter["schema"],
                )
            }));
            let expected: Vec<(String, u64)> = expected
                .into_iter()
                .map(|(name, max)| (name.to_owned(), max))
                .collect();
            assert_eq!(declared, expected);
        }
    }

    fn schema_of<S: PartialSchema>() -> Value {
        serde_json::to_value(S::schema()).expect("serializes")
    }

    #[test]
    fn body_and_response_schemas_declare_lengths_and_the_token_pattern() {
        let request = schema_of::<CardUpdateRequest>();
        let properties = request["properties"].as_object().expect("properties");
        let declared: BTreeSet<(String, u64)> = max_lengths(
            properties
                .iter()
                .map(|(name, schema)| (name.as_str(), schema)),
        )
        .into_iter()
        .collect();
        let expected: BTreeSet<(String, u64)> = [
            ("account_id", 11),
            ("embossed_name", 50),
            ("active_status", 1),
            ("expiry_month", 2),
            ("expiry_year", 4),
        ]
        .into_iter()
        .map(|(name, max)| (name.to_owned(), max))
        .collect();
        assert_eq!(declared, expected);
        assert_eq!(properties["version"]["pattern"], "^[0-9a-f]{16}$");
        assert!(request.get("required").is_none(), "{request}");

        let response = schema_of::<CardResponse>();
        let names: BTreeSet<&str> = response["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect();
        let shown = BTreeSet::from([
            "card_number",
            "account_id",
            "card_embossed_name",
            "card_expiraion_date",
            "card_active_status",
            "version",
        ]);
        assert_eq!(names, shown);
        assert_eq!(
            response["properties"]["version"]["pattern"],
            "^[0-9a-f]{16}$"
        );
        let required: BTreeSet<&str> = response["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(required, shown);
    }

    /// Asserts that `field`'s serde name and `as_str` are `name`, that the
    /// name deserializes back to `field`, and that its rank is `rank`.
    fn assert_bms_field<F>(field: F, name: &str, rank: u16)
    where
        F: BmsField + Serialize + DeserializeOwned + PartialEq + Debug,
    {
        assert_eq!(field.as_str(), name);
        assert_eq!(serde_json::to_value(field).expect("serializes"), name);
        let back: F = serde_json::from_value(Value::from(name)).expect("deserializes");
        assert_eq!(back, field);
        assert_eq!(field.rank(), rank, "{name}");
    }

    #[test]
    fn field_enums_name_the_bms_fields_of_the_audit_rows() {
        for (field, name, rank) in [
            (CardListField::AcctSid, "ACCTSID", 1),
            (CardListField::CardSid, "CARDSID", 2),
        ] {
            assert_eq!(field.as_str(), name);
            assert_bms_field(field, name, rank);
        }
        for (field, name, rank) in [
            (CardViewField::AcctSid, "ACCTSID", 1),
            (CardViewField::CardSid, "CARDSID", 3),
        ] {
            assert_eq!(field.as_str(), name);
            assert_bms_field(field, name, rank);
        }
        for (field, name, rank) in [
            (CardUpdateField::AcctSid, "ACCTSID", 2),
            (CardUpdateField::CardSid, "CARDSID", 4),
            (CardUpdateField::CrdName, "CRDNAME", 8),
            (CardUpdateField::CrdStCd, "CRDSTCD", 11),
            (CardUpdateField::ExpMon, "EXPMON", 13),
            (CardUpdateField::ExpYear, "EXPYEAR", 15),
        ] {
            assert_eq!(field.as_str(), name);
            assert_bms_field(field, name, rank);
        }
        // (published schema, its enum values)
        let schemas = [
            (schema_of::<CardListField>(), json!(["ACCTSID", "CARDSID"])),
            (schema_of::<CardViewField>(), json!(["ACCTSID", "CARDSID"])),
            (
                schema_of::<CardUpdateField>(),
                json!([
                    "ACCTSID", "CARDSID", "CRDNAME", "CRDSTCD", "EXPMON", "EXPYEAR"
                ]),
            ),
        ];
        for (schema, names) in schemas {
            assert_eq!(schema["enum"], names, "{schema}");
        }
    }
}
