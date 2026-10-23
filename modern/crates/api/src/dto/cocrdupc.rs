//! COCRDUPC, the card update: the contracts of endpoint 9, `GET /api/v1/cards/edit` (ENTER
//! fetch), and endpoint 10, `PUT /api/v1/cards/{card_ref}` (ENTER validate, F5 save)
//! (transaction `CCUP`, map `CCRDUPA`, route `/cards/update`; AAP 0.3.4.1). F3 and F12 are the
//! screen's own and send no request (COCRDUPC-004, COCRDUPC-006).
//!
//! # Wire contract
//!
//! - Endpoint 9 takes the query string of endpoint 8,
//!   [`CardLookupQuery`](super::cocrdslc::CardLookupQuery), and answers [`CardUpdateView`]: the
//!   card `9000-READ-DATA` fetched and the [`CardUpdateSnapshot`] of `CCUP-OLD-DETAILS` in state
//!   `S` (COCRDUPC-002, COCRDUPC-016, COCRDUPC-017, COCRDUPC-044).
//! - Endpoint 10 takes [`CardUpdateRequest`]: the key, the raw text of the four detail fields and
//!   the echoed snapshot. The card is the `card_ref` of the path, which the handler passes on
//!   unparsed and the service resolves (COCRDUPC-043). It answers [`CardUpdateResult`]: the info
//!   line, the state the screen enters, whether `F5=Save F12=Cancel` is shown, whether the card
//!   was rewritten, and the snapshot to echo next (COCRDUPC-034, COCRDUPC-035, COCRDUPC-039).
//! - Every other outcome is a `ScreenError` from `crate::error`, with the status the audit
//!   assigns: 400 for an edit or no change, 404 for no card, 409 for a card changed by another
//!   user, 500 for a store failure. Its `field` is a member name of [`CardUpdateFields`] or of
//!   the query.
//!
//! # State
//!
//! COCRDUPC keeps `CCUP-CHANGE-ACTION` and `CCUP-OLD-DETAILS` in its COMMAREA between tasks
//! (`COCRDUPC.cbl:274-301`). Endpoint 10 is stateless, so both travel in the
//! [`CardUpdateSnapshot`]: its [`CardUpdateState`] and the values fetched. The client echoes the
//! snapshot of the last 200 unchanged; after an error response it sets only `state`, to the state
//! that error leaves. The snapshot grants nothing: a save re-runs the change check and the edits
//! on the fields received and compares the card read for update with it (COCRDUPC-041,
//! COCRDUPC-044, `X-COCRDUPC-1`). A state other than the seven the program sets does not
//! deserialise, so it is refused here with a 400 before the domain is reached (`X-COCRDUPC-8`).
//!
//! # Cardholder data
//!
//! - The card number leaves only as `CardNumber`, whose `Serialize` and `Debug` give twelve `*`
//!   and the last four digits (COCRDUPC-042, `E32`). The snapshot holds no card number: the card
//!   is the path's `card_ref`.
//! - No DTO here has a CVV field, and no schema property or JSON key names one (COCRDUPC-042).
//!   `CCUP-OLD-DETAILS` keeps `CCUP-OLD-CVV-CD`, and `9300-CHECK-CHANGE-IN-REC` compares it with
//!   the `CARD-CVV-CD` read for update (`COCRDUPC.cbl:291-301,1498-1520`), but a CVV cannot
//!   travel to the client under Cardholder Data Redaction. The CVV term is settled server-side:
//!   `domain::online::cocrdupc::check_change_in_rec` drops it (`X-COCRDUPC-4`) and the rewrite
//!   keeps the stored CVV. Any change to that comparison belongs to the domain and the service,
//!   as an escalation of the `E32` family, and never to a field of this module.
//!
//! # Raw input and typed output
//!
//! The expiry month and year of a request are the raw text of `EXPMON` and `EXPYEAR`, separate
//! numeric screen fields that `1250-EDIT-EXPIRY-MON` and `1260-EDIT-EXPIRY-YEAR` edit with their
//! own messages, the raw-edit-input exception of Typed Dates and Timestamps (COCRDUPC-029 to
//! COCRDUPC-032). Every expiry date the API sends, in the view and in the snapshot, is a typed
//! date, `null` for a blank stored date; the day of the dark `EXPDAY` field travels only inside
//! the snapshot's date (COCRDUPC-009, COCRDUPC-017).
//!
//! # Validation
//!
//! The DTOs enforce only the map lengths, the server half of the `both` rows COCRDUP-019
//! (`CRDNAME`, 50), COCRDUP-022 (`CRDSTCD`, 1), COCRDUP-025 (`EXPMON`, 2) and COCRDUP-027
//! (`EXPYEAR`, 4), and the widths of the snapshot's fields: a longer value is a 400 naming
//! `fields.<member>` or `snapshot.<member>`. Every other edit and its message is COCRDUPC's, run
//! by the service through `domain::online::cocrdupc::decide`. Both endpoints return a single
//! resource and take no `page_size`.

use chrono::NaiveDate;
use domain::online::cocrdupc as rules;
use domain::sensitive::CardNumber;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::{digits, full_text};

/// The key of an endpoint 10 request.
///
/// `ENTER` runs the change check and the edits and asks for confirmation; `PF5` saves, and only
/// when the echoed `snapshot.state` is `changes_ok_not_confirmed`: in any other state it is
/// processed as `ENTER` (COCRDUPC-007). The screen submits every other key as `ENTER` and keeps
/// F3 and F12 to itself (COCRDUPC-008), so any other key name is a 400.
// The schema name `CocrdupcAid` keeps the program's key enum apart from the other programs'
// (Generated API Types Only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = CocrdupcAid)]
pub enum Aid {
    /// ENTER: the change check and the edits, then the confirmation prompt.
    #[serde(rename = "ENTER")]
    Enter,
    /// F5: the save of the changes validated in state `N`.
    #[serde(rename = "PF5")]
    Pf5,
}

impl From<Aid> for rules::Aid {
    fn from(aid: Aid) -> Self {
        match aid {
            Aid::Enter => Self::Enter,
            Aid::Pf5 => Self::Pf5,
        }
    }
}

/// `CCUP-CHANGE-ACTION PIC X(1)` (`COCRDUPC.cbl:276-290`): the state a response leaves the
/// screen in, one value per code the program sets (COCRDUPC-044).
///
/// The screen derives from it which fields are enterable and which keys are shown
/// (COCRDUPC-045, COCRDUPC-046), and echoes it in `snapshot.state`. A value outside these seven
/// does not deserialise, which refuses the codes the program never sets (`X-COCRDUPC-8`).
// The schema name `CardUpdateState` is fixed (Generated API Types Only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardUpdateState {
    /// Spaces or low-values, `CCUP-DETAILS-NOT-FETCHED`: the search screen.
    NotFetched,
    /// `S`, `CCUP-SHOW-DETAILS`: a card is shown and its details are enterable.
    ShowDetails,
    /// `E`, `CCUP-CHANGES-NOT-OK`: an edit failed.
    ChangesNotOk,
    /// `N`, `CCUP-CHANGES-OK-NOT-CONFIRMED`: the changes passed and await F5.
    ChangesOkNotConfirmed,
    /// `C`, `CCUP-CHANGES-OKAYED-AND-DONE`: the card was rewritten.
    ChangesOkayedAndDone,
    /// `L`, `CCUP-CHANGES-OKAYED-LOCK-ERROR`: the read for update failed.
    LockError,
    /// `F`, `CCUP-CHANGES-OKAYED-BUT-FAILED`: the rewrite failed.
    UpdateFailed,
}

impl From<rules::CardUpdateState> for CardUpdateState {
    fn from(state: rules::CardUpdateState) -> Self {
        use rules::CardUpdateState as S;
        match state {
            S::NotFetched => Self::NotFetched,
            S::ShowDetails => Self::ShowDetails,
            S::ChangesNotOk => Self::ChangesNotOk,
            S::ChangesOkNotConfirmed => Self::ChangesOkNotConfirmed,
            S::ChangesOkayedAndDone => Self::ChangesOkayedAndDone,
            S::LockError => Self::LockError,
            S::UpdateFailed => Self::UpdateFailed,
        }
    }
}

impl From<CardUpdateState> for rules::CardUpdateState {
    fn from(state: CardUpdateState) -> Self {
        use CardUpdateState as S;
        match state {
            S::NotFetched => Self::NotFetched,
            S::ShowDetails => Self::ShowDetails,
            S::ChangesNotOk => Self::ChangesNotOk,
            S::ChangesOkNotConfirmed => Self::ChangesOkNotConfirmed,
            S::ChangesOkayedAndDone => Self::ChangesOkayedAndDone,
            S::LockError => Self::LockError,
            S::UpdateFailed => Self::UpdateFailed,
        }
    }
}

/// The raw text of the four enterable detail fields of an endpoint 10 request, at most the map
/// `LENGTH` of each (COCRDUP-019, COCRDUP-022, COCRDUP-025, COCRDUP-027).
///
/// An absent member is the empty text, which the program reads as blank (COCRDUPC-009). The
/// screen zero-fills a shorter month or year on the left before sending it (COCRDUPC-033). The
/// expiry day is no input: it is the snapshot's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[serde(default)]
pub struct CardUpdateFields {
    /// `CRDNAME` (`CRDNAMEI PIC X(50)`): the name on the card, edited by `1230-EDIT-NAME`.
    #[validate(length(max = 50))]
    #[schema(max_length = 50, example = "TEST HOLDER")]
    pub embossed_name: String,
    /// `CRDSTCD` (`CRDSTCDI PIC X(1)`): the active status, `Y` or `N`, edited by
    /// `1240-EDIT-CARDSTATUS`.
    #[validate(length(max = 1))]
    #[schema(max_length = 1, example = "Y")]
    pub status: String,
    /// `EXPMON` (`EXPMONI PIC X(2)`): the expiry month, edited by `1250-EDIT-EXPIRY-MON`.
    #[validate(length(max = 2))]
    #[schema(max_length = 2, example = "06")]
    pub expiry_month: String,
    /// `EXPYEAR` (`EXPYEARI PIC X(4)`): the expiry year, edited by `1260-EDIT-EXPIRY-YEAR`.
    #[validate(length(max = 4))]
    #[schema(max_length = 4, example = "2030")]
    pub expiry_year: String,
}

impl CardUpdateFields {
    /// The fields as COCRDUPC's input: each member the raw text `1100-RECEIVE-MAP` receives,
    /// which the domain's edits own (COCRDUPC-009, COCRDUPC-023 to COCRDUPC-032).
    #[must_use]
    pub fn input(&self) -> rules::CardUpdateInput<'_> {
        rules::CardUpdateInput {
            embossed_name: &self.embossed_name,
            status: &self.status,
            expiry_month: &self.expiry_month,
            expiry_year: &self.expiry_year,
        }
    }
}

/// `CCUP-OLD-DETAILS` and `CCUP-CHANGE-ACTION`: the values endpoint 9 fetched and the state the
/// last response left, which the client echoes in every endpoint 10 request (COCRDUPC-044).
///
/// It holds the account as entered, the name upper-cased, the typed expiry date with its day and
/// the status; the name and status keep every byte of their fields, trailing spaces included
/// (COCRDUPC-017). It holds no card number, because the card is the path's `card_ref`, and no
/// card verification code, which cannot leave the service (module docs). A value the client
/// alters equals no stored value, so it reads as a change and, at a save, as a card changed by
/// another user: the snapshot grants nothing (`X-COCRDUPC-1`).
// `state` travels with the fetched values because the domain decides what a key does from the
// echoed state (`rules::CardSnapshotFields::state`), and a typed member refuses a code the
// program never sets before the domain is reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct CardUpdateSnapshot {
    /// `CCUP-CHANGE-ACTION`: the state the last response left.
    pub state: CardUpdateState,
    /// `CCUP-OLD-ACCTID PIC X(11)`: the account entered for the search or the list row's, 11
    /// digits, which a save writes into the card (COCRDUPC-038, `X-COCRDUPC-2`).
    #[validate(length(max = 11))]
    #[schema(max_length = 11, example = "00000000050")]
    pub acct_id: String,
    /// `CCUP-OLD-CRDNAME PIC X(50)`: the stored name, upper-cased at the fetch, all 50 bytes
    /// (COCRDUPC-017).
    #[validate(length(max = 50))]
    #[schema(
        max_length = 50,
        example = "TEST HOLDER                                       "
    )]
    pub embossed_name: String,
    /// `CCUP-OLD-EXPYEAR`, `CCUP-OLD-EXPMON` and `CCUP-OLD-EXPDAY`: the stored expiry date,
    /// `null` when it is blank.
    pub expiration_date: Option<NaiveDate>,
    /// `CCUP-OLD-CRDSTCD PIC X(1)`: the stored active status.
    #[validate(length(max = 1))]
    #[schema(max_length = 1, example = "Y")]
    pub status: String,
}

/// The snapshot as the text the client echoes, the name and status every byte of their fields
/// (COCRDUPC-017, COCRDUPC-044). An account the domain holds as unknown, from an echo that was
/// not 11 digits, is the empty text, which reads back as unknown.
impl From<rules::CardSnapshot> for CardUpdateSnapshot {
    fn from(snapshot: rules::CardSnapshot) -> Self {
        Self {
            state: snapshot.state().into(),
            acct_id: snapshot.acct_id().map_or_else(String::new, digits),
            embossed_name: full_text(snapshot.embossed_name()),
            expiration_date: snapshot.expiration_date(),
            status: full_text(snapshot.status()),
        }
    }
}

/// The echoed snapshot as COCRDUPC's `CCUP-OLD-DETAILS`, through the domain's total
/// `rules::CardSnapshot::from_fields`: any text yields a snapshot.
impl From<&CardUpdateSnapshot> for rules::CardSnapshot {
    fn from(snapshot: &CardUpdateSnapshot) -> Self {
        Self::from_fields(&rules::CardSnapshotFields {
            state: snapshot.state.into(),
            acct_id: &snapshot.acct_id,
            embossed_name: &snapshot.embossed_name,
            expiration_date: snapshot.expiration_date,
            status: &snapshot.status,
        })
    }
}

/// The 200 body of endpoint 9: the card as `3200-SETUP-SCREEN-VARS` shows it in state `S`, with
/// the snapshot the client echoes (COCRDUPC-002, COCRDUPC-016, COCRDUPC-017, COCRDUPC-044).
///
/// The card number is redacted and the card verification code absent (COCRDUPC-042, `E32`).
/// The screen shows the month and year of `expiration_date` in `EXPMON` and `EXPYEAR` and keeps
/// the day out of sight.
// No `Deserialize`: `CardNumber` serialises redacted only and cannot be read back. The schema
// name `CardUpdateView` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct CardUpdateView {
    /// The card's `cards.card_ref` handle, which endpoint 10 takes in its path (COCRDUPC-043).
    #[schema(example = 1)]
    pub card_ref: i64,
    /// `ACCTSIDO`: the account as entered, or the list row's, 11 digits with their leading
    /// zeros; never the card's own `CARD-ACCT-ID` (COCRDUPC-016, `X-COCRDUPC-2`).
    #[schema(example = "00000000050")]
    pub acct_id: String,
    /// `CARDSIDO`: the card number, twelve `*` followed by its last four digits (COCRDUPC-042).
    #[schema(value_type = String, example = "************5740")]
    pub card_num: CardNumber,
    /// `CRDNAMEO`: the stored name with `a` to `z` upper-cased and every other byte unchanged,
    /// all 50 bytes, trailing spaces included (COCRDUPC-017).
    #[schema(example = "TEST HOLDER                                       ")]
    pub embossed_name: String,
    /// `CARD-EXPIRAION-DATE PIC X(10)` as a date, `null` when the stored date is blank
    /// (COCRDUPC-017, CVACT02Y-005).
    pub expiration_date: Option<NaiveDate>,
    /// `CRDSTCDO` from `CARD-ACTIVE-STATUS PIC X(01)`: the stored byte, a space included
    /// (COCRDUPC-017).
    #[schema(example = "Y")]
    pub status: String,
    /// The fetched values in state `show_details`, echoed in every endpoint 10 request
    /// (COCRDUPC-044).
    pub snapshot: CardUpdateSnapshot,
    /// `ERRMSGO`: always `null`, because a fetched card has no error message; every other
    /// outcome is a `ScreenError` (COCRDUPC-002, COCRDUPC-016).
    pub message: Option<String>,
    /// `INFOMSGO` (40): `Details of selected card shown above`, the info line of state `S`
    /// (COCRDUPC-016).
    pub info_message: Option<String>,
}

/// The fields of the domain's view, rendered as the map shows them.
impl From<rules::CardUpdateView> for CardUpdateView {
    fn from(view: rules::CardUpdateView) -> Self {
        let rules::CardUpdateView {
            card_ref,
            card_num,
            acct_id,
            embossed_name,
            expiration_date,
            status,
            snapshot,
            info_message,
        } = view;
        Self {
            card_ref: card_ref.get(),
            acct_id: digits(acct_id),
            card_num,
            embossed_name: full_text(&embossed_name),
            expiration_date,
            status: full_text(&status),
            snapshot: snapshot.into(),
            message: None,
            info_message: Some(info_message.to_owned()),
        }
    }
}

/// The body of endpoint 10: the key, the raw text of the detail fields and the echoed snapshot,
/// whose `state` the last response set (COCRDUPC-044).
///
/// The handler passes it to the service owned; the service builds the domain's input with
/// [`CardUpdateFields::input`] and its snapshot through `From<&CardUpdateSnapshot>`.
// The schema name `CardUpdateRequest` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({
    "aid": "ENTER",
    "fields": {
        "embossed_name": "TEST HOLDER",
        "status": "N",
        "expiry_month": "06",
        "expiry_year": "2030"
    },
    "snapshot": {
        "state": "show_details",
        "acct_id": "00000000050",
        "embossed_name": "TEST HOLDER                                       ",
        "expiration_date": "2025-04-30",
        "status": "Y"
    }
}))]
pub struct CardUpdateRequest {
    /// `ENTER` validates, `PF5` saves.
    pub aid: Aid,
    /// The raw text of the four detail fields.
    #[serde(deserialize_with = "super::object")]
    #[validate(nested)]
    pub fields: CardUpdateFields,
    /// The snapshot of the fetch, with the state of the last response.
    #[serde(deserialize_with = "super::object")]
    #[validate(nested)]
    pub snapshot: CardUpdateSnapshot,
}

/// The 200 body of endpoint 10: the screen `3000-SEND-MAP` sends after a validation, a save or
/// a restart, none of which has an error message (COCRDUPC-034, COCRDUPC-035, COCRDUPC-039).
///
/// The fields keep the text the request sent. `snapshot` is what the client echoes next, already
/// in `state`; it is `null` after a restart, when the screen searches again through endpoint 9.
// `rules::CardUpdateOutcome` carries the snapshot and the state, never the card's number or
// handle, so the result holds those and no `CardUpdateView`: a 200 of endpoint 10 changes no
// fetched value, and F5 saves only once the client echoes `changes_ok_not_confirmed`. The info
// line is `info_message`, as in `CardUpdateView`, because `message` is the error row there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[schema(example = json!({
    "info_message": "Changes validated.Press F5 to save",
    "state": "changes_ok_not_confirmed",
    "save_enabled": true,
    "committed": false,
    "snapshot": {
        "state": "changes_ok_not_confirmed",
        "acct_id": "00000000050",
        "embossed_name": "TEST HOLDER                                       ",
        "expiration_date": "2025-04-30",
        "status": "Y"
    }
}))]
pub struct CardUpdateResult {
    /// `INFOMSGO` (40): the info line of the state the response leaves, `Changes validated.Press
    /// F5 to save`, `Changes committed to database` or `Please enter Account and Card Number`
    /// (COCRDUPC-034, COCRDUPC-039).
    pub info_message: String,
    /// `CCUP-CHANGE-ACTION` after the response: `changes_ok_not_confirmed` after a validation,
    /// `changes_okayed_and_done` after a save, `not_fetched` after a restart.
    pub state: CardUpdateState,
    /// Whether `F5=Save F12=Cancel` is shown: with the confirmation prompt only (COCRDUPC-034).
    pub save_enabled: bool,
    /// Whether the card was rewritten (COCRDUPC-039).
    pub committed: bool,
    /// The snapshot to echo next; `null` after a restart.
    pub snapshot: Option<CardUpdateSnapshot>,
}

/// The outcome's state decides the info line, the state and the keys; its snapshot is the one
/// the client echoes. Every text is the domain's.
impl From<rules::CardUpdateOutcome> for CardUpdateResult {
    fn from(outcome: rules::CardUpdateOutcome) -> Self {
        let state = outcome.state();
        let (committed, snapshot) = match outcome {
            rules::CardUpdateOutcome::Validated { snapshot } => (false, Some(snapshot.into())),
            rules::CardUpdateOutcome::Committed { snapshot } => (true, Some(snapshot.into())),
            rules::CardUpdateOutcome::Restart => (false, None),
        };
        Self {
            info_message: state.info_message().to_owned(),
            state: state.into(),
            save_enabled: state.confirmation_keys_shown(),
            committed,
            snapshot,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::header::CONTENT_TYPE;
    use axum::http::{Request, StatusCode};
    use chrono::NaiveDate;
    use domain::copybook::{PicU, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdupc as rules;
    use domain::sensitive::CardNumber;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};
    use validator::Validate;

    use super::{
        Aid, CardUpdateFields, CardUpdateRequest, CardUpdateResult, CardUpdateSnapshot,
        CardUpdateState, CardUpdateView,
    };
    use crate::dto::{FIELD_LENGTH_MESSAGE, INVALID_BODY, ValidJson};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The status, message and field of a rejected body.
    type Rejection = (StatusCode, String, Option<String>);

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1111222233334444";

    /// [`CARD`] as twelve `*` and its last four digits.
    const REDACTED: &str = "************4444";

    /// The handle a COCRDLI list row carries.
    const HANDLE: CardRef = CardRef::new(7);

    /// Every state with its wire name and its COBOL code.
    const STATES: [(CardUpdateState, &str, char); 7] = [
        (CardUpdateState::NotFetched, "not_fetched", ' '),
        (CardUpdateState::ShowDetails, "show_details", 'S'),
        (CardUpdateState::ChangesNotOk, "changes_not_ok", 'E'),
        (
            CardUpdateState::ChangesOkNotConfirmed,
            "changes_ok_not_confirmed",
            'N',
        ),
        (
            CardUpdateState::ChangesOkayedAndDone,
            "changes_okayed_and_done",
            'C',
        ),
        (CardUpdateState::LockError, "lock_error", 'L'),
        (CardUpdateState::UpdateFailed, "update_failed", 'F'),
    ];

    fn expiry() -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2025, 4, 30).ok_or_else(|| "2025-04-30 is a date".into())
    }

    /// What `9000-READ-DATA` keeps of an active card named `TEST HOLDER` (stored as `Test
    /// Holder`) and expiring `expiry`, fetched under account 50, in state `S`.
    fn snapshot(expiry: Option<NaiveDate>) -> rules::CardSnapshot {
        rules::CardSnapshot::from_fields(&rules::CardSnapshotFields {
            state: rules::CardUpdateState::ShowDetails,
            acct_id: "00000000050",
            embossed_name: "TEST HOLDER",
            expiration_date: expiry,
            status: "Y",
        })
    }

    /// Endpoint 9's view of that card, as `rules::fetch` builds it for handle 7.
    fn fetched(expiry: Option<NaiveDate>) -> Result<rules::CardUpdateView, Box<dyn StdError>> {
        Ok(rules::CardUpdateView {
            card_ref: HANDLE,
            card_num: CardNumber::parse(CARD)?,
            acct_id: PicU::new(50)?,
            embossed_name: PicX::from_text("TEST HOLDER")?,
            expiration_date: expiry,
            status: PicX::from_text("Y")?,
            snapshot: snapshot(expiry),
            info_message: rules::FOUND_CARDS_FOR_ACCOUNT,
        })
    }

    /// Changed values that pass every edit.
    fn valid_change() -> CardUpdateFields {
        CardUpdateFields {
            embossed_name: "New Holder".to_owned(),
            status: "N".to_owned(),
            expiry_month: "06".to_owned(),
            expiry_year: "2030".to_owned(),
        }
    }

    /// `body` extracted through [`ValidJson`], as the handler receives it, a rejection as its
    /// status, message and field.
    async fn extract(
        body: &Value,
    ) -> Result<Result<CardUpdateRequest, Rejection>, Box<dyn StdError>> {
        let request = Request::builder()
            .method("PUT")
            .uri("/api/v1/cards/7")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))?;
        Ok(ValidJson::<CardUpdateRequest>::from_request(request, &())
            .await
            .map(|ValidJson(value)| value)
            .map_err(|error| {
                (
                    error.status(),
                    error.message().to_owned(),
                    error.field().map(str::to_owned),
                )
            }))
    }

    /// `body` extracted through [`ValidJson`], which must accept it.
    async fn accept(body: &Value) -> Result<CardUpdateRequest, Box<dyn StdError>> {
        extract(body)
            .await?
            .map_err(|(_, message, _)| message.into())
    }

    /// The rejection of a 400 with `message`, naming `field`.
    fn bad_request(message: &str, field: Option<&str>) -> Rejection {
        (
            StatusCode::BAD_REQUEST,
            message.to_owned(),
            field.map(str::to_owned),
        )
    }

    /// Every key of `value` and of the objects and arrays it holds, lower-cased.
    fn keys(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    out.push(key.to_lowercase());
                    keys(inner, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|inner| keys(inner, out)),
            _ => {}
        }
    }

    /// The decision of `decide` for `request` over the snapshot it echoes.
    fn decide(request: &CardUpdateRequest) -> Result<rules::Decision, rules::CardUpdateFailure> {
        rules::decide(
            request.aid.into(),
            &rules::CardSnapshot::from(&request.snapshot),
            &request.fields.input(),
        )
    }

    // audit: COCRDUPC-042
    // audit: COCRDUPC-044
    #[test]
    fn the_view_shows_the_fetched_card_redacted_with_its_snapshot() -> TestResult {
        let view = CardUpdateView::from(fetched(Some(expiry()?))?);
        let snapshot = json!({
            "state": "show_details",
            "acct_id": "00000000050",
            "embossed_name": "TEST HOLDER                                       ",
            "expiration_date": "2025-04-30",
            "status": "Y",
        });
        assert_eq!(
            serde_json::to_value(&view)?,
            json!({
                "card_ref": 7,
                "acct_id": "00000000050",
                "card_num": REDACTED,
                "embossed_name": "TEST HOLDER                                       ",
                "expiration_date": "2025-04-30",
                "status": "Y",
                "snapshot": snapshot,
                "message": null,
                "info_message": "Details of selected card shown above",
            })
        );
        let debug = format!("{view:?}");
        assert!(!debug.contains(CARD), "the card number leaked into Debug");
        assert!(debug.contains(REDACTED));
        Ok(())
    }

    // audit: COCRDUPC-017
    // audit: COCRDUPC-044
    #[test]
    fn the_name_and_status_travel_as_every_byte_of_their_fields() -> TestResult {
        let held = rules::CardSnapshot::from_fields(&rules::CardSnapshotFields {
            state: rules::CardUpdateState::ShowDetails,
            acct_id: "00000000050",
            embossed_name: "TEST HOLDER",
            expiration_date: None,
            status: " ",
        });
        let view = CardUpdateView::from(rules::CardUpdateView {
            status: PicX::spaces(),
            snapshot: held.clone(),
            ..fetched(None)?
        });
        let name = format!("{:<50}", "TEST HOLDER");
        for (shown_name, shown_status) in [
            (&view.embossed_name, &view.status),
            (&view.snapshot.embossed_name, &view.snapshot.status),
        ] {
            assert_eq!((shown_name, shown_status.as_str()), (&name, " "));
        }
        // The 50 and 1 bytes pass the snapshot's widths and rebuild the snapshot held.
        assert!(view.snapshot.validate().is_ok());
        assert_eq!(rules::CardSnapshot::from(&view.snapshot), held);
        Ok(())
    }

    // audit: COCRDUPC-017
    #[test]
    fn a_blank_expiry_date_is_null_in_the_view_and_the_snapshot() -> TestResult {
        let view = CardUpdateView::from(fetched(None)?);
        assert_eq!(
            (view.expiration_date, view.snapshot.expiration_date),
            (None, None)
        );
        let body = serde_json::to_value(&view)?;
        assert_eq!(body["expiration_date"], Value::Null);
        assert_eq!(body["snapshot"]["expiration_date"], Value::Null);
        Ok(())
    }

    #[test]
    fn every_state_travels_by_name_and_maps_to_its_program_code() -> TestResult {
        for (state, wire, code) in STATES {
            assert_eq!(serde_json::to_value(state)?, json!(wire));
            assert_eq!(
                serde_json::from_value::<CardUpdateState>(json!(wire))?,
                state
            );
            let program = rules::CardUpdateState::from(state);
            assert_eq!(program.code(), code, "{wire}");
            assert_eq!(CardUpdateState::from(program), state);
        }
        for unknown in ["S", "", " ", "X", "Show_Details", "SHOW_DETAILS"] {
            assert!(
                serde_json::from_value::<CardUpdateState>(json!(unknown)).is_err(),
                "{unknown:?}"
            );
        }
        let schema = serde_json::to_value(CardUpdateState::schema())?;
        assert_eq!(
            schema["enum"],
            json!(STATES.map(|(_, wire, _)| wire).to_vec())
        );
        assert_eq!(CardUpdateState::name(), "CardUpdateState");
        Ok(())
    }

    #[test]
    fn the_key_travels_as_enter_or_pf5() -> TestResult {
        for (aid, wire, program) in [
            (Aid::Enter, "ENTER", rules::Aid::Enter),
            (Aid::Pf5, "PF5", rules::Aid::Pf5),
        ] {
            assert_eq!(serde_json::to_value(aid)?, json!(wire));
            assert_eq!(serde_json::from_value::<Aid>(json!(wire))?, aid);
            assert_eq!(rules::Aid::from(aid), program);
        }
        for unknown in ["PF3", "PF12", "enter", ""] {
            assert!(
                serde_json::from_value::<Aid>(json!(unknown)).is_err(),
                "{unknown:?}"
            );
        }
        assert_eq!(Aid::name(), "CocrdupcAid");
        Ok(())
    }

    // audit: COCRDUPC-044
    #[test]
    fn the_echoed_snapshot_rebuilds_the_fetched_one() -> TestResult {
        for expiry in [Some(expiry()?), None] {
            let fetched = fetched(expiry)?.snapshot;
            let echoed = CardUpdateSnapshot::from(fetched.clone());
            assert_eq!(rules::CardSnapshot::from(&echoed), fetched);
            // Through JSON too: the text the client holds rebuilds the same snapshot.
            let back: CardUpdateSnapshot = serde_json::from_value(serde_json::to_value(&echoed)?)?;
            assert_eq!(back, echoed);
            for (state, _, _) in STATES {
                let moved = CardUpdateSnapshot {
                    state,
                    ..echoed.clone()
                };
                assert_eq!(
                    rules::CardSnapshot::from(&moved),
                    fetched.clone().with_state(state.into())
                );
            }
        }
        // An account that is not 11 digits is held as unknown and echoed as the empty text.
        let altered = CardUpdateSnapshot {
            acct_id: "123".to_owned(),
            ..CardUpdateSnapshot::from(fetched(None)?.snapshot)
        };
        let held = rules::CardSnapshot::from(&altered);
        assert_eq!(held.acct_id(), None);
        assert_eq!(CardUpdateSnapshot::from(held).acct_id, "");
        Ok(())
    }

    // audit: COCRDUPC-009
    #[test]
    fn the_fields_reach_the_domain_as_the_raw_text_sent() {
        let fields = CardUpdateFields {
            embossed_name: " Mixed case ".to_owned(),
            status: "y".to_owned(),
            expiry_month: "3".to_owned(),
            expiry_year: String::new(),
        };
        assert_eq!(
            fields.input(),
            rules::CardUpdateInput {
                embossed_name: " Mixed case ",
                status: "y",
                expiry_month: "3",
                expiry_year: "",
            }
        );
        assert_eq!(
            CardUpdateFields::default().input(),
            rules::CardUpdateInput::default()
        );
    }

    // audit: COCRDUP-019
    // audit: COCRDUP-022
    // audit: COCRDUP-025
    // audit: COCRDUP-027
    #[tokio::test]
    async fn the_body_is_deserialised_then_held_to_the_map_lengths() -> TestResult {
        let snapshot =
            serde_json::to_value(CardUpdateSnapshot::from(fetched(Some(expiry()?))?.snapshot))?;
        let request = accept(&json!({
            "aid": "PF5",
            "fields": serde_json::to_value(valid_change())?,
            "snapshot": snapshot,
        }))
        .await?;
        assert_eq!((request.aid, &request.fields), (Aid::Pf5, &valid_change()));
        assert_eq!(request.snapshot.state, CardUpdateState::ShowDetails);

        // Absent detail fields are the empty text, which the domain's blank rules answer.
        let empty = accept(&json!({"aid": "ENTER", "fields": {}, "snapshot": snapshot})).await?;
        assert_eq!(empty.fields, CardUpdateFields::default());

        let full = valid_change();
        let cases = [
            (
                CardUpdateFields {
                    embossed_name: "A".repeat(51),
                    ..full.clone()
                },
                "fields.embossed_name",
            ),
            (
                CardUpdateFields {
                    status: "YN".to_owned(),
                    ..full.clone()
                },
                "fields.status",
            ),
            (
                CardUpdateFields {
                    expiry_month: "012".to_owned(),
                    ..full.clone()
                },
                "fields.expiry_month",
            ),
            (
                CardUpdateFields {
                    expiry_year: "02030".to_owned(),
                    ..full
                },
                "fields.expiry_year",
            ),
        ];
        for (fields, field) in cases {
            let error = extract(&json!({"aid": "ENTER", "fields": fields, "snapshot": snapshot}))
                .await?
                .err()
                .ok_or("an over-long field was accepted")?;
            assert_eq!(error, bad_request(FIELD_LENGTH_MESSAGE, Some(field)));
        }
        // The full map lengths pass.
        let widest = CardUpdateFields {
            embossed_name: "A".repeat(50),
            status: "Y".to_owned(),
            expiry_month: "12".to_owned(),
            expiry_year: "2099".to_owned(),
        };
        assert!(widest.validate().is_ok());

        let mut long_account = snapshot.clone();
        long_account["acct_id"] = json!("000000000500");
        let error =
            extract(&json!({"aid": "ENTER", "fields": valid_change(), "snapshot": long_account}))
                .await?
                .err()
                .ok_or("an over-long snapshot account was accepted")?;
        assert_eq!(
            error,
            bad_request(FIELD_LENGTH_MESSAGE, Some("snapshot.acct_id"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_unknown_state_or_key_is_an_unreadable_body() -> TestResult {
        let snapshot = serde_json::to_value(CardUpdateSnapshot::from(fetched(None)?.snapshot))?;
        let mut unknown_state = snapshot.clone();
        unknown_state["state"] = json!("X");
        let mut missing_state = snapshot.clone();
        missing_state
            .as_object_mut()
            .ok_or("the snapshot is an object")?
            .remove("state");
        for body in [
            json!({"aid": "ENTER", "fields": {}, "snapshot": unknown_state}),
            json!({"aid": "ENTER", "fields": {}, "snapshot": missing_state}),
            json!({"aid": "PF12", "fields": {}, "snapshot": snapshot}),
            json!({"aid": "ENTER", "fields": {}}),
        ] {
            let error = extract(&body)
                .await?
                .err()
                .ok_or("an unreadable body was accepted")?;
            assert_eq!(error, bad_request(INVALID_BODY, None));
        }
        Ok(())
    }

    // audit: COCRDUPC-043
    #[tokio::test]
    async fn a_card_number_sent_beside_the_body_is_never_read_or_kept() -> TestResult {
        let snapshot = serde_json::to_value(CardUpdateSnapshot::from(fetched(None)?.snapshot))?;
        let request = accept(&json!({
            "aid": "ENTER",
            "fields": {},
            "snapshot": snapshot,
            "card_num": CARD,
        }))
        .await?;
        assert!(!format!("{request:?}").contains(CARD));
        assert!(!serde_json::to_string(&request)?.contains(CARD));
        Ok(())
    }

    // audit: COCRDUPC-034
    // audit: COCRDUPC-039
    // audit: COCRDUPC-041
    #[test]
    fn a_validation_then_a_save_answer_with_the_snapshot_to_echo() -> TestResult {
        let view = CardUpdateView::from(fetched(Some(expiry()?))?);
        let enter = CardUpdateRequest {
            aid: Aid::Enter,
            fields: valid_change(),
            snapshot: view.snapshot.clone(),
        };
        let rules::Decision::Respond(outcome) = decide(&enter)? else {
            return Err("ENTER saved".into());
        };
        let validated = CardUpdateResult::from(outcome);
        let echoed = CardUpdateSnapshot {
            state: CardUpdateState::ChangesOkNotConfirmed,
            ..view.snapshot
        };
        assert_eq!(
            validated,
            CardUpdateResult {
                info_message: "Changes validated.Press F5 to save".to_owned(),
                state: CardUpdateState::ChangesOkNotConfirmed,
                save_enabled: true,
                committed: false,
                snapshot: Some(echoed.clone()),
            }
        );

        // F5 with the snapshot the validation returned asks for the save.
        let save = CardUpdateRequest {
            aid: Aid::Pf5,
            snapshot: validated
                .snapshot
                .ok_or("a validation returns its snapshot")?,
            ..enter
        };
        let rules::Decision::Save(effects) = decide(&save)? else {
            return Err("F5 in state N did not save".into());
        };
        let committed = CardUpdateResult::from(effects.committed());
        let done = CardUpdateSnapshot {
            state: CardUpdateState::ChangesOkayedAndDone,
            ..echoed
        };
        assert_eq!(
            serde_json::to_value(&committed)?,
            json!({
                "info_message": "Changes committed to database",
                "state": "changes_okayed_and_done",
                "save_enabled": false,
                "committed": true,
                "snapshot": done,
            })
        );
        Ok(())
    }

    #[test]
    fn a_restart_answers_the_search_screen_without_a_snapshot() -> TestResult {
        let restart = CardUpdateResult::from(rules::CardUpdateOutcome::Restart);
        assert_eq!(
            serde_json::to_value(&restart)?,
            json!({
                "info_message": "Please enter Account and Card Number",
                "state": "not_fetched",
                "save_enabled": false,
                "committed": false,
                "snapshot": null,
            })
        );
        Ok(())
    }

    // audit: COCRDUPC-042
    // audit: COCRDUPC-043
    #[test]
    fn no_body_or_schema_names_a_cvv_or_carries_a_full_number() -> TestResult {
        let view = CardUpdateView::from(fetched(Some(expiry()?))?);
        let result = CardUpdateResult::from(rules::CardUpdateOutcome::Validated {
            snapshot: fetched(None)?.snapshot,
        });
        let request = CardUpdateRequest {
            aid: Aid::Enter,
            fields: valid_change(),
            snapshot: view.snapshot.clone(),
        };
        let bodies = [
            serde_json::to_value(&view)?,
            serde_json::to_value(&result)?,
            serde_json::to_value(&request)?,
        ];
        let schemas = [
            serde_json::to_value(Aid::schema())?,
            serde_json::to_value(CardUpdateState::schema())?,
            serde_json::to_value(CardUpdateFields::schema())?,
            serde_json::to_value(CardUpdateSnapshot::schema())?,
            serde_json::to_value(CardUpdateView::schema())?,
            serde_json::to_value(CardUpdateRequest::schema())?,
            serde_json::to_value(CardUpdateResult::schema())?,
        ];
        let mut found = Vec::new();
        for value in bodies.iter().chain(&schemas) {
            keys(value, &mut found);
            assert!(!value.to_string().contains(CARD), "a full card number left");
        }
        assert!(found.iter().all(|key| !key.contains("cvv")), "{found:?}");

        // The snapshot names no card number: the card is the path's `card_ref`.
        let snapshot_schema = serde_json::to_value(CardUpdateSnapshot::schema())?;
        assert_eq!(snapshot_schema["properties"].get("card_num"), None);

        let view_schema = serde_json::to_value(CardUpdateView::schema())?;
        let card_num = &view_schema["properties"]["card_num"];
        assert_eq!(card_num["type"], json!("string"));
        assert_eq!(card_num["example"], json!("************5740"));
        for schema in [&view_schema, &snapshot_schema] {
            assert_eq!(
                schema["properties"]["expiration_date"]["format"],
                json!("date")
            );
        }
        Ok(())
    }

    #[test]
    fn the_schema_names_are_the_contract() {
        assert_eq!(
            [
                CardUpdateFields::name(),
                CardUpdateSnapshot::name(),
                CardUpdateView::name(),
                CardUpdateRequest::name(),
                CardUpdateResult::name(),
            ],
            [
                "CardUpdateFields",
                "CardUpdateSnapshot",
                "CardUpdateView",
                "CardUpdateRequest",
                "CardUpdateResult",
            ]
        );
    }
}
