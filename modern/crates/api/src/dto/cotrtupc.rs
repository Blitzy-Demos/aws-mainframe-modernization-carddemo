//! The contracts of endpoints 26 and 27, transaction-type maintenance over the Db2 data version
//! (transaction `CTTU`, program COTRTUPC, map `CTRTUPA`, admin only; AAP 0.3.4.1):
//! `GET /api/v1/transaction-types/{type_code}` answers [`TranTypeView`], and
//! `POST /api/v1/transaction-types/{type_code}/actions` takes [`TranTypeActionRequest`] and
//! answers [`TranTypeActionResult`].
//!
//! Every edit, state rule and text is `domain::online::cotrtupc`'s; this module only converts
//! (Thin Request Handlers). The `{type_code}` path parameter is the raw text of
//! `TRTYPCDI PIC X(2)`, which the handler passes on unparsed and the domain stores and edits
//! (COTRTUPC-004 to COTRTUPC-008).
//!
//! # State
//!
//! COTRTUPC keeps `TTUP-CHANGE-ACTION` in its COMMAREA between screens. Both responses return it
//! as [`TranTypeMaintState`], and the screen holds it between requests, as the COMMAREA did,
//! together with the states it enters without a request: the confirmation prompt of the first
//! F4, the add of F5 in state X and the F12 cancellations (COTRTUPC-015, COTRTUPC-031,
//! COTRTUPC-039 to COTRTUPC-041). The enum therefore names all sixteen states, including those
//! no response returns.
//!
//! The request does not carry the state back. Endpoint 27 serves each key in the state the audit's
//! endpoint mapping gives it, derived on the server from the key and the row the service re-reads
//! for `{type_code}` ([`served_state`](rules::served_state)): ENTER in S or R, PF5 in N, PF4 in 9.
//! An echoed state could therefore decide nothing and authorise nothing: before every write the
//! service re-reads the row and the domain edits the input again (COTRTUPC-024). A body that
//! still sends a `state` member is accepted, the member ignored.
//!
//! # Errors
//!
//! An edit or store failure is a [`MaintFailure`](rules::MaintFailure), which the service returns
//! as its error and `api::error` turns into a [`ScreenError`](crate::error::ScreenError) with the
//! audited status: 400 for the edits and an unserved key, 500 for every store failure, the
//! delete refused while categories exist (COTRTUPC-033) and the lock error (COTRTUPC-029)
//! included. No outcome is a 404 or a 409: a type no row holds is a 200 offering the add path
//! (COTRTUPC-010), and the changed-by-another-user text is never set (COTRTUPC-025). A
//! store-failure message is the program's fixed prefix only; the Db2 text after it is
//! `HALTED(H3)` (COTRTUPC-049).
//!
//! No value this program handles is protected (Cardholder Data Redaction).

use domain::online::cotrtupc as rules;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// The key of an endpoint 27 request (`EIBAID`): `ENTER`, `PF4` or `PF5` (COTRTUPC-043).
///
/// Any other key name the client sends is accepted, so that it gets the program's own answer,
/// 400 `Invalid key pressed` with nothing read or written, never the rejection of an unreadable
/// body. F3 and F12 are the screen's, which serves them without a request (COTRTUPC-039 to
/// COTRTUPC-041, COTRTUPC-044).
// The schema name `CotrtupcAid` keeps it apart from every other program's key enum in
// `components.schemas` (Generated API Types Only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "UPPERCASE")]
#[schema(as = CotrtupcAid)]
pub enum Aid {
    /// `ENTER`: validate the description entered for the code shown (COTRTUPC-016 to
    /// COTRTUPC-021).
    Enter,
    /// `PF4`: the second F4, which deletes the type shown once the screen has asked for the
    /// confirmation (COTRTUPC-032 to COTRTUPC-034).
    Pf4,
    /// `PF5`: save the validated description, an update, or an insert when no row holds the
    /// code (COTRTUPC-023 to COTRTUPC-029).
    Pf5,
    /// Any other key name. Absent from the schema and never serialised: the screen never sends
    /// it, and serialising it is an error rather than a value the server would read differently.
    #[serde(other, skip_serializing)]
    Unserved,
}

impl From<Aid> for rules::TtupAid {
    fn from(aid: Aid) -> Self {
        match aid {
            Aid::Enter => Self::Enter,
            Aid::Pf4 => Self::Pf4,
            Aid::Pf5 => Self::Pf5,
            // F12 is the one key of the domain that endpoint 27 does not serve; `decide` answers
            // it with `Invalid key pressed` before reading anything, which is the answer
            // COTRTUPC-043 gives every key but ENTER, PF4 and PF5.
            Aid::Unserved => Self::Pf12,
        }
    }
}

/// `TTUP-CHANGE-ACTION PIC X(1)` (`COTRTUPC.cbl:296-327`): the state a response leaves the
/// screen in, one value per value the program's 88-levels name.
///
/// The screen keeps it until the next request and derives from it the keys it shows, the fields
/// it protects and, after an error response, the info row (COTRTUPC-045, COTRTUPC-046). It carries
/// no authority, and no request sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranTypeMaintState {
    /// Low-values or spaces, `TTUP-DETAILS-NOT-FETCHED`: the fresh screen (COTRTUPC-003).
    DetailsNotFetched,
    /// `K`, `TTUP-INVALID-SEARCH-KEYS`: replaced by the statement after the one that sets it.
    InvalidSearchKeys,
    /// `X`, `TTUP-DETAILS-NOT-FOUND`: no row holds the code; F5 starts the add (COTRTUPC-010).
    DetailsNotFound,
    /// `S`, `TTUP-SHOW-DETAILS`: a fetched row shown, its description editable (COTRTUPC-009).
    ShowDetails,
    /// `R`, `TTUP-CREATE-NEW-RECORD`: a new type being entered (COTRTUPC-015).
    CreateNewRecord,
    /// `V`, `TTUP-REVIEW-NEW-RECORD`: declared and never set.
    ReviewNewRecord,
    /// `9`, `TTUP-CONFIRM-DELETE`: a delete awaiting its confirming F4 (COTRTUPC-031).
    ConfirmDelete,
    /// `8`, `TTUP-START-DELETE`: the state a delete refused by categories leaves (COTRTUPC-033).
    StartDelete,
    /// `7`, `TTUP-DELETE-DONE`: the type was deleted (COTRTUPC-032).
    DeleteDone,
    /// `6`, `TTUP-DELETE-FAILED` (COTRTUPC-034).
    DeleteFailed,
    /// `E`, `TTUP-CHANGES-NOT-OK`: an entered description failed an edit (COTRTUPC-018).
    ChangesNotOk,
    /// `N`, `TTUP-CHANGES-OK-NOT-CONFIRMED`: validated, awaiting F5 (COTRTUPC-020).
    ChangesOkNotConfirmed,
    /// `L`, `TTUP-CHANGES-OKAYED-LOCK-ERROR`: the update could not lock the row (COTRTUPC-029).
    ChangesOkayedLockError,
    /// `F`, `TTUP-CHANGES-OKAYED-BUT-FAILED`: unreachable (COTRTUPC-028).
    ChangesOkayedButFailed,
    /// `C`, `TTUP-CHANGES-OKAYED-AND-DONE`: the save ended, committed or not (COTRTUPC-024,
    /// COTRTUPC-027, COTRTUPC-028).
    ChangesOkayedAndDone,
    /// `B`, `TTUP-CHANGES-BACKED-OUT`: F12 in state N (COTRTUPC-039).
    ChangesBackedOut,
}

impl From<rules::TtupState> for TranTypeMaintState {
    fn from(state: rules::TtupState) -> Self {
        use rules::TtupState as S;
        match state {
            S::DetailsNotFetched => Self::DetailsNotFetched,
            S::InvalidSearchKeys => Self::InvalidSearchKeys,
            S::DetailsNotFound => Self::DetailsNotFound,
            S::ShowDetails => Self::ShowDetails,
            S::CreateNewRecord => Self::CreateNewRecord,
            S::ReviewNewRecord => Self::ReviewNewRecord,
            S::ConfirmDelete => Self::ConfirmDelete,
            S::StartDelete => Self::StartDelete,
            S::DeleteDone => Self::DeleteDone,
            S::DeleteFailed => Self::DeleteFailed,
            S::ChangesNotOk => Self::ChangesNotOk,
            S::ChangesOkNotConfirmed => Self::ChangesOkNotConfirmed,
            S::ChangesOkayedLockError => Self::ChangesOkayedLockError,
            S::ChangesOkayedButFailed => Self::ChangesOkayedButFailed,
            S::ChangesOkayedAndDone => Self::ChangesOkayedAndDone,
            S::ChangesBackedOut => Self::ChangesBackedOut,
        }
    }
}

/// The 200 of endpoint 26, the fetch: the screen ENTER sends in the fresh or X state.
///
/// A row found gives state `show_details` with the stored code and description and the search
/// prompt as info row (COTRTUPC-009). A code no row holds gives state `details_not_found` with
/// the normalised code, no description, error row `No record found for this key in database`
/// and info row `Press F05 to add. F12 to cancel` (COTRTUPC-010).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "type_code": "01",
    "description": "PURCHASE",
    "message": null,
    "info_message": "Enter transaction type to be maintained",
    "state": "show_details"
}))]
pub struct TranTypeView {
    /// `TRTYPCDO PIC X(2)`, the code shown without trailing spaces: the stored code of a row
    /// found, or the normalised key of a code no row holds (COTRTUPC-008). The screen sends it
    /// back as `{type_code}` of endpoint 27.
    pub type_code: String,
    /// `TRTYDSCO PIC X(50)`: the stored description up to its stored length, trailing spaces
    /// kept, since a value saved through COTRTUPC is stored padded to 50 (COTRTUPC-009,
    /// COTRTUPC-023). Absent when no row holds the code.
    pub description: Option<String>,
    /// The error row, `WS-RETURN-MSG`, exactly as the program words it; absent when empty.
    pub message: Option<String>,
    /// The info row, `WS-INFO-MSG`, for the state the screen enters (COTRTUPC-045); the screen
    /// centres it in `INFOMSG`.
    pub info_message: String,
    /// The state the screen enters, which it holds until the next request.
    pub state: TranTypeMaintState,
}

impl From<rules::TranTypeView> for TranTypeView {
    fn from(view: rules::TranTypeView) -> Self {
        Self {
            type_code: super::text(view.code()),
            description: view.description().map(str::to_owned),
            message: view.message().map(str::to_owned),
            info_message: view.info_message().to_owned(),
            state: view.state().into(),
        }
    }
}

/// The body of endpoint 27: the key, and the description `TRTYDSCI PIC X(50)` as typed.
///
/// The description is the raw map input: COTRTUPC stores it and owns its edits, the required
/// and alphanumeric tests and their messages (COTRTUPC-004, COTRTUPC-018, COTRTUPC-019). Only
/// the map `LENGTH` is checked here, the server half of `COTRTUP-016`; a longer value is a 400
/// naming `description`. Absent, it is the empty text, as the domain reads an absent field.
// `{ aid, description? }` as AAP 0.3.4.1 and the audit's endpoint mapping give it: no member
// echoes the screen's state, because the service derives the state it serves (module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({"aid": "ENTER", "description": "PURCHASES"}))]
pub struct TranTypeActionRequest {
    /// The key pressed.
    pub aid: Aid,
    /// `TRTYDSCI PIC X(50)`, as typed: up to 50 characters.
    #[validate(length(max = 50))]
    #[schema(max_length = 50)]
    pub description: Option<String>,
}

impl TranTypeActionRequest {
    /// The two map inputs the domain receives: `type_code`, the raw `{type_code}` path
    /// parameter, and the description as typed, the empty text when absent.
    #[must_use]
    pub fn input<'a>(&'a self, type_code: &'a str) -> rules::MaintInput<'a> {
        rules::MaintInput {
            type_code,
            description: self.description.as_deref().unwrap_or_default(),
        }
    }
}

/// The 200 of endpoint 27: the screen the step sends, and whether it wrote.
///
/// ENTER with a valid change gives state `changes_ok_not_confirmed` and info row
/// `Changes validated.Press F5 to save`, nothing written (COTRTUPC-020). PF5 gives state
/// `changes_okayed_and_done`, info row `Changes committed to database`, after the update or the
/// insert that follows an update finding no row (COTRTUPC-024, COTRTUPC-026). PF4 gives state
/// `delete_done`, info row `Delete successful.` (COTRTUPC-032).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "type_code": "01",
    "description": "PURCHASES                                         ",
    "message": null,
    "info_message": "Changes validated.Press F5 to save",
    "state": "changes_ok_not_confirmed",
    "committed": false,
    "deleted": false
}))]
pub struct TranTypeActionResult {
    /// `TRTYPCDO PIC X(2)`, the code shown without trailing spaces; empty on the fresh screen.
    pub type_code: String,
    /// `TRTYDSCO PIC X(50)`: the description validated or saved, the `FUNCTION TRIM` of the
    /// typed text padded to 50 characters, which is also the value stored (COTRTUPC-020,
    /// COTRTUPC-023). Absent after a delete, whose screen keeps the fetched values the screen
    /// already holds (COTRTUPC-030), and on the fresh screen.
    pub description: Option<String>,
    /// The error row, `WS-RETURN-MSG`, exactly as the program words it; absent when empty.
    pub message: Option<String>,
    /// The info row, `WS-INFO-MSG`, for the state the screen enters (COTRTUPC-045).
    pub info_message: String,
    /// The state the screen enters, which it holds until the next request.
    pub state: TranTypeMaintState,
    /// Whether the step wrote to `db2.transaction_type` and the service committed it: a save or
    /// a delete. The info row alone does not tell: a failed save also leaves state
    /// `changes_okayed_and_done` (COTRTUPC-027, COTRTUPC-028).
    pub committed: bool,
    /// Whether the committed write was the delete of the type shown (COTRTUPC-032).
    pub deleted: bool,
}

impl TranTypeActionResult {
    /// A screen that wrote nothing: `type_code` and `description` shown, error row `message`
    /// and the info row of `state`.
    fn unwritten(
        type_code: String,
        description: Option<String>,
        message: Option<String>,
        state: rules::TtupState,
    ) -> Self {
        Self {
            type_code,
            description,
            message,
            info_message: state.info_message().to_owned(),
            state: state.into(),
            committed: false,
            deleted: false,
        }
    }
}

/// The screen of a step that ended without a failure. A screen in state C or 7 is only ever the
/// answer to a write the store completed (`rules::apply_result` with `StoreResult::Done`), which
/// the service commits before answering.
impl From<rules::TranTypeView> for TranTypeActionResult {
    fn from(view: rules::TranTypeView) -> Self {
        let state = view.state();
        let deleted = state == rules::TtupState::DeleteDone;
        Self {
            committed: deleted || state == rules::TtupState::ChangesOkayedAndDone,
            deleted,
            ..Self::unwritten(
                super::text(view.code()),
                view.description().map(str::to_owned),
                view.message().map(str::to_owned),
                state,
            )
        }
    }
}

/// The answer to a step of endpoint 27, for every outcome `rules::decide` and
/// `rules::apply_result` can return.
///
/// The service runs every [`MaintEffect`](rules::MaintEffect) through `rules::apply_result` and
/// returns a [`MaintFailure`](rules::MaintFailure) as its error, so that the response carries the
/// failure's audited status; the handler converts only what remains. The conversion is total
/// all the same, each outcome rendered as the screen it stands for, with nothing committed:
///
/// - `Screen`: that screen, as the conversion from `rules::TranTypeView` gives it.
/// - `Failed`: the failure's error row and the info row of the state it leaves, the entered
///   values not repeated.
/// - `Execute`: the write not yet run, in the state that asked for it: the validated values in
///   state N for an update or insert, the code in state 9 for a delete.
/// - `Abend`: the abend message `ABEND-ROUTINE` sends, on the fresh screen the next visit opens;
///   the conversation ends and nothing is written (COTRTUPC-037).
impl From<rules::MaintOutcome> for TranTypeActionResult {
    fn from(outcome: rules::MaintOutcome) -> Self {
        match outcome {
            rules::MaintOutcome::Screen(view) => view.into(),
            rules::MaintOutcome::Failed(failure) => Self::unwritten(
                String::new(),
                None,
                error_row(failure.message()),
                failure.state(),
            ),
            rules::MaintOutcome::Execute(
                rules::MaintEffect::Update { code, description }
                | rules::MaintEffect::Insert { code, description },
            ) => Self::unwritten(
                super::text(&code),
                Some(description),
                None,
                rules::TtupState::ChangesOkNotConfirmed,
            ),
            rules::MaintOutcome::Execute(rules::MaintEffect::Delete { code }) => Self::unwritten(
                super::text(&code),
                None,
                None,
                rules::TtupState::ConfirmDelete,
            ),
            rules::MaintOutcome::Abend(data) => Self::unwritten(
                String::new(),
                None,
                error_row(&super::text(&data.msg)),
                rules::TtupState::DetailsNotFetched,
            ),
        }
    }
}

/// An error row as a response carries it: absent when the program leaves it blank, as for the
/// unexpected failure whose `ABEND-MSG` stays spaces (COTRTUPC-038).
fn error_row(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use domain::copybook::PicX;
    use domain::online::cotrtupc as rules;
    use domain::records::db2_transaction_type::Db2TransactionType;
    use rules::{MaintEffect, MaintOutcome, StoreResult, TtupAid, TtupState, TypeLookup};
    use serde_json::json;
    use utoipa::{PartialSchema, ToSchema};
    use validator::Validate;

    use super::{
        Aid, TranTypeActionRequest, TranTypeActionResult, TranTypeMaintState, TranTypeView,
    };

    type TestResult = Result<(), Box<dyn StdError>>;

    // The audited literals, written out apart from the domain's constants.
    const SEARCH: &str = "Enter transaction type to be maintained";
    const ADD_PROMPT: &str = "Press F05 to add. F12 to cancel";
    const VALIDATED: &str = "Changes validated.Press F5 to save";
    const COMMITTED: &str = "Changes committed to database";
    const DELETE_PROMPT: &str = "Delete this record ? Press F4 to confirm";
    const DELETED: &str = "Delete successful.";
    const NOT_FOUND: &str = "No record found for this key in database";
    const NO_CHANGE: &str = "No change detected with respect to values fetched.";
    const INVALID_KEY: &str = "Invalid key pressed";
    const UPDATE_ERROR: &str = "Error updating: TRANSACTION_TYPE Table. SQLCODE:";

    /// Every domain state with its wire name, in declaration order.
    const STATES: [(TtupState, &str); 16] = [
        (TtupState::DetailsNotFetched, "details_not_fetched"),
        (TtupState::InvalidSearchKeys, "invalid_search_keys"),
        (TtupState::DetailsNotFound, "details_not_found"),
        (TtupState::ShowDetails, "show_details"),
        (TtupState::CreateNewRecord, "create_new_record"),
        (TtupState::ReviewNewRecord, "review_new_record"),
        (TtupState::ConfirmDelete, "confirm_delete"),
        (TtupState::StartDelete, "start_delete"),
        (TtupState::DeleteDone, "delete_done"),
        (TtupState::DeleteFailed, "delete_failed"),
        (TtupState::ChangesNotOk, "changes_not_ok"),
        (TtupState::ChangesOkNotConfirmed, "changes_ok_not_confirmed"),
        (
            TtupState::ChangesOkayedLockError,
            "changes_okayed_lock_error",
        ),
        (
            TtupState::ChangesOkayedButFailed,
            "changes_okayed_but_failed",
        ),
        (TtupState::ChangesOkayedAndDone, "changes_okayed_and_done"),
        (TtupState::ChangesBackedOut, "changes_backed_out"),
    ];

    /// The seeded row `01`, `PURCHASE` (`DB2LTTYP.ctl:17`).
    fn purchase() -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            PicX::<2>::from_text("01")?,
            "PURCHASE".to_owned(),
        )?)
    }

    /// `text` space-padded to the 50 bytes of `TTUP-NEW-TTYP-TYPE-DESC`.
    fn padded(text: &str) -> String {
        format!("{text:<50}")
    }

    /// The outcome of endpoint 27 for `aid` in the state the service serves it in, the row `01`
    /// re-read, with `description` typed.
    fn step(aid: TtupAid, description: &str) -> Result<MaintOutcome, Box<dyn StdError>> {
        let row = purchase()?;
        let current = TypeLookup::Found(&row);
        let input = rules::MaintInput {
            type_code: "01",
            description,
        };
        Ok(rules::decide(
            aid,
            rules::served_state(aid, current),
            &input,
            current,
        ))
    }

    #[test]
    fn aid_travels_as_the_three_served_keys() -> TestResult {
        for (aid, wire, domain) in [
            (Aid::Enter, "\"ENTER\"", TtupAid::Enter),
            (Aid::Pf4, "\"PF4\"", TtupAid::Pf4),
            (Aid::Pf5, "\"PF5\"", TtupAid::Pf5),
        ] {
            assert_eq!(serde_json::to_string(&aid)?, wire);
            assert_eq!(serde_json::from_str::<Aid>(wire)?, aid);
            assert_eq!(TtupAid::from(aid), domain);
        }
        assert_eq!(Aid::name(), "CotrtupcAid");
        assert_eq!(
            serde_json::to_value(Aid::schema())?["enum"],
            json!(["ENTER", "PF4", "PF5"])
        );
        assert!(serde_json::to_string(&Aid::Unserved).is_err());
        Ok(())
    }

    #[test]
    fn any_other_key_is_answered_invalid_key_pressed() -> TestResult {
        for wire in ["\"PF12\"", "\"PF3\"", "\"PF10\"", "\"enter\"", "\"\""] {
            let aid = serde_json::from_str::<Aid>(wire)?;
            assert_eq!(aid, Aid::Unserved, "{wire}");
            let failure = match step(aid.into(), "PURCHASES")? {
                MaintOutcome::Failed(failure) => failure,
                other => return Err(format!("{wire} gave {other:?}").into()),
            };
            assert_eq!(failure.message(), INVALID_KEY, "{wire}");
            assert_eq!(failure.class().http_status(), 400, "{wire}");
        }
        for body in [json!({"aid": 5}), json!({"aid": null}), json!({})] {
            assert!(serde_json::from_value::<TranTypeActionRequest>(body).is_err());
        }
        Ok(())
    }

    #[test]
    fn every_program_state_has_its_own_wire_name() -> TestResult {
        let mut seen = Vec::new();
        for (state, wire) in STATES {
            let shown = TranTypeMaintState::from(state);
            assert_eq!(serde_json::to_value(shown)?, json!(wire));
            assert_eq!(
                serde_json::from_value::<TranTypeMaintState>(json!(wire))?,
                shown
            );
            assert!(!seen.contains(&shown), "{wire}");
            seen.push(shown);
        }
        assert_eq!(
            serde_json::to_value(TranTypeMaintState::schema())?["enum"],
            json!(STATES.map(|(_, wire)| wire))
        );
        Ok(())
    }

    #[test]
    fn the_fetch_shows_the_row_or_offers_the_add_path() -> TestResult {
        let row = purchase()?;
        let found = TranTypeView::from(rules::fetch(
            &PicX::from_text("01")?,
            TypeLookup::Found(&row),
        )?);
        assert_eq!(
            serde_json::to_value(&found)?,
            json!({
                "type_code": "01",
                "description": "PURCHASE",
                "message": null,
                "info_message": SEARCH,
                "state": "show_details"
            })
        );
        let missing =
            TranTypeView::from(rules::fetch(&PicX::from_text("09")?, TypeLookup::NotFound)?);
        assert_eq!(
            missing,
            TranTypeView {
                type_code: "09".to_owned(),
                description: None,
                message: Some(NOT_FOUND.to_owned()),
                info_message: ADD_PROMPT.to_owned(),
                state: TranTypeMaintState::DetailsNotFound,
            }
        );
        // A description saved through COTRTUPC is stored padded to 50 and shown so.
        let saved = Db2TransactionType::new(PicX::from_text("08")?, padded("NEW TYPE"))?;
        let shown = TranTypeView::from(rules::fetch(
            &PicX::from_text("08")?,
            TypeLookup::Found(&saved),
        )?);
        assert_eq!(shown.description, Some(padded("NEW TYPE")));
        Ok(())
    }

    #[test]
    fn the_request_checks_the_map_length_only() -> TestResult {
        for (description, valid) in [
            (None, true),
            (Some(String::new()), true),
            (Some("*".to_owned()), true),
            (Some("9".repeat(50)), true),
            (Some("é".repeat(50)), true),
            (Some("X".repeat(51)), false),
        ] {
            let request = TranTypeActionRequest {
                aid: Aid::Pf5,
                description,
            };
            match request.validate() {
                Ok(()) => assert!(valid, "{request:?}"),
                Err(errors) => {
                    assert!(!valid, "{request:?}");
                    assert!(errors.field_errors().contains_key("description"));
                }
            }
        }
        let absent = TranTypeActionRequest {
            aid: Aid::Enter,
            description: None,
        };
        assert_eq!(
            absent.input(" 1"),
            rules::MaintInput {
                type_code: " 1",
                description: "",
            }
        );
        // A screen that echoes its state is still served; the member is ignored.
        let echoed: TranTypeActionRequest = serde_json::from_value(json!({
            "aid": "PF4", "description": "PURCHASE", "state": "confirm_delete"
        }))?;
        assert_eq!(echoed.aid, Aid::Pf4);
        assert_eq!(echoed.input("01").description, "PURCHASE");
        Ok(())
    }

    /// A result as the screen reads it: code, description, error row, info row, state,
    /// committed, deleted.
    type Shown = (
        String,
        Option<String>,
        Option<String>,
        String,
        TranTypeMaintState,
        bool,
        bool,
    );

    fn shown(outcome: MaintOutcome) -> Shown {
        let result = TranTypeActionResult::from(outcome);
        (
            result.type_code,
            result.description,
            result.message,
            result.info_message,
            result.state,
            result.committed,
            result.deleted,
        )
    }

    fn expected(
        code: &str,
        description: Option<&str>,
        message: Option<&str>,
        info: &str,
        state: TranTypeMaintState,
        flags: (bool, bool),
    ) -> Shown {
        (
            code.to_owned(),
            description.map(str::to_owned),
            message.map(str::to_owned),
            info.to_owned(),
            state,
            flags.0,
            flags.1,
        )
    }

    /// Asserts each outcome converts to its expected screen.
    fn assert_shown(cases: Vec<(MaintOutcome, Shown)>) {
        for (outcome, want) in cases {
            let label = format!("{outcome:?}");
            assert_eq!(shown(outcome), want, "{label}");
        }
    }

    #[test]
    fn enter_validates_or_finds_no_change_and_writes_nothing() -> TestResult {
        let saved = padded("PURCHASES");
        assert_shown(vec![
            (
                step(TtupAid::Enter, "  PURCHASES ")?,
                expected(
                    "01",
                    Some(&saved),
                    None,
                    VALIDATED,
                    TranTypeMaintState::ChangesOkNotConfirmed,
                    (false, false),
                ),
            ),
            // The stored description, case aside, is no change.
            (
                step(TtupAid::Enter, "purchase")?,
                expected(
                    "",
                    None,
                    Some(NO_CHANGE),
                    SEARCH,
                    TranTypeMaintState::ShowDetails,
                    (false, false),
                ),
            ),
        ]);
        Ok(())
    }

    #[test]
    fn a_save_is_committed_only_when_the_store_completes_it() -> TestResult {
        let saved = padded("PURCHASES");
        let update = MaintEffect::Update {
            code: PicX::from_text("01")?,
            description: saved.clone(),
        };
        let pending = expected(
            "01",
            Some(&saved),
            None,
            VALIDATED,
            TranTypeMaintState::ChangesOkNotConfirmed,
            (false, false),
        );
        assert_shown(vec![
            // PF5 asks for the update, which the service runs before answering.
            (step(TtupAid::Pf5, "PURCHASES")?, pending),
            (
                rules::apply_result(update.clone(), StoreResult::Done),
                expected(
                    "01",
                    Some(&saved),
                    None,
                    COMMITTED,
                    TranTypeMaintState::ChangesOkayedAndDone,
                    (true, false),
                ),
            ),
            // A failed update leaves state C beside its error, with nothing committed.
            (
                rules::apply_result(update, StoreResult::OtherError),
                expected(
                    "",
                    None,
                    Some(UPDATE_ERROR),
                    COMMITTED,
                    TranTypeMaintState::ChangesOkayedAndDone,
                    (false, false),
                ),
            ),
        ]);
        Ok(())
    }

    #[test]
    fn a_delete_is_committed_and_reported_deleted() -> TestResult {
        let delete = MaintEffect::Delete {
            code: PicX::from_text("01")?,
        };
        assert_shown(vec![
            // PF4 asks for the delete of the code shown, which the service runs.
            (
                step(TtupAid::Pf4, "")?,
                expected(
                    "01",
                    None,
                    None,
                    DELETE_PROMPT,
                    TranTypeMaintState::ConfirmDelete,
                    (false, false),
                ),
            ),
            (
                rules::apply_result(delete, StoreResult::Done),
                expected(
                    "01",
                    None,
                    None,
                    DELETED,
                    TranTypeMaintState::DeleteDone,
                    (true, true),
                ),
            ),
        ]);
        Ok(())
    }

    #[test]
    fn the_abend_ends_on_the_fresh_screen() -> TestResult {
        let row = purchase()?;
        let current = TypeLookup::Found(&row);
        let input = rules::MaintInput {
            type_code: "01",
            description: "PURCHASE",
        };
        // ENTER after a refused delete finds no change and no branch for state 8 (COTRTUPC-037).
        let outcome = rules::decide(TtupAid::Enter, TtupState::StartDelete, &input, current);
        assert!(matches!(outcome, MaintOutcome::Abend(_)));
        assert_eq!(
            shown(outcome),
            expected(
                "",
                None,
                Some("UNEXPECTED DATA SCENARIO"),
                SEARCH,
                TranTypeMaintState::DetailsNotFetched,
                (false, false)
            )
        );
        Ok(())
    }

    #[test]
    fn schema_names_are_the_contract() {
        assert_eq!(TranTypeView::name(), "TranTypeView");
        assert_eq!(TranTypeActionRequest::name(), "TranTypeActionRequest");
        assert_eq!(TranTypeActionResult::name(), "TranTypeActionResult");
        assert_eq!(TranTypeMaintState::name(), "TranTypeMaintState");
        assert_eq!(Aid::name(), "CotrtupcAid");
    }
}
