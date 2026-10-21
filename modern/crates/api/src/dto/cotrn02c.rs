//! Endpoint 13, `POST /api/v1/transactions`: the transaction add contract of transaction `CT02`,
//! program COTRN02C, map `COTRN2A` (AAP 0.3.4.1).
//!
//! The request carries the attention key, the account or card key, the eleven data fields and
//! the confirmation, each the raw text of its map field (COTRN02C-060). ENTER runs the edits and,
//! with confirmation `Y` or `y`, adds the transaction; PF5 copies the data fields of the
//! transaction with the highest ID, then runs ENTER on them. F3 and F4 are the screen's own and
//! send no request (COTRN02C-004, COTRN02C-005).
//!
//! # Rules applied
//!
//! - Cardholder Data Redaction: a typed card number is write-only input, never serialised and
//!   masked in `Debug`. The card the program fills in from the cross-reference leaves as
//!   [`CardNumber`], whose serialisation is the redacted form (COTRN02C-012, COTRN02C-017,
//!   COTRN02C-057). Every example is redacted.
//! - Typed Dates and Timestamps: the original and processing dates of the request stay raw text,
//!   because COTRN02C's shape edits and the CSUTLDTC validity call own their messages (the
//!   raw-edit-input exception of AAP 0.3.4.1; COTRN02C-034, COTRN02C-035, COTRN02C-037,
//!   COTRN02C-038). The dates a response carries are typed, `format: date`.
//! - R1: the amount travels as text in both directions, the request's as typed and the
//!   response's as COTRN02C re-edited it (COTRN02C-036); nothing here computes with it.
//! - The DTOs check only the map `LENGTH` of each input, the server half of rows `COTRN02-013` to
//!   `COTRN02-057` of `modern/audit/screens.md`. Which key decides, every emptiness and digit
//!   edit and the confirmation are COTRN02C's edits in `domain::online::cotrn02c`.
//!
//! # Attention keys
//!
//! [`Aid`] admits `ENTER` and `PF5` only. Any other value fails deserialisation and is answered
//! by `ValidJson` with 400 and no field, before any read. COTRN02C-007 words that answer with
//! the 50-byte invalid-key text; the message differs here, because neither the rejection of
//! `ValidJson` nor COTRN02C's `AddFailure` can carry that text.

use core::fmt;

use chrono::NaiveDate;
use domain::copybook::PicX;
use domain::online::cotrn02c as rules;
use domain::sensitive::CardNumber;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::{digits, text};

/// What the `Debug` of a [`TranAddRequest`] prints for a typed card number: sixteen `*`. The
/// input need not be a card number at all, so not even its last four characters are shown.
const CARD_INPUT_REDACTED: &str = "****************";

/// The attention key of an endpoint 13 request, `TranAddRequest.aid` (COTRN02C-007).
// The schema name `Cotrn02cAid` is fixed (Generated API Types Only): each program declares its
// own `Aid`, so the bare name would collide in `components.schemas`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "UPPERCASE")]
#[schema(as = Cotrn02cAid)]
pub enum Aid {
    /// `ENTER`, `PROCESS-ENTER-KEY`: the key edits, the data edits on the request's fields, then
    /// the confirmation (COTRN02C-009 to COTRN02C-042).
    Enter,
    /// `PF5`, `COPY-LAST-TRAN-DATA`: the key edits, the copy of the data fields of the
    /// transaction with the highest ID, then ENTER on the copied fields (COTRN02C-052 to
    /// COTRN02C-056).
    Pf5,
}

/// The key as `domain::online::cotrn02c` names it.
impl From<Aid> for rules::Aid {
    fn from(aid: Aid) -> Self {
        match aid {
            Aid::Enter => Self::Enter,
            Aid::Pf5 => Self::Pf5,
        }
    }
}

/// The eleven data fields of `COTRN2A`, each the raw text of its map field and at most its map
/// `LENGTH`; an absent member is the empty text (COTRN02C-060).
///
/// The service hands them to COTRN02C's edits unchanged, the dates and the amount included.
/// On PF5 the program replaces them with the copied transaction's, whatever they hold
/// (COTRN02C-054).
// The schema name `TranAddFields` is fixed (Generated API Types Only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({
    "type_cd": "01",
    "cat_cd": "0001",
    "source": "POS TERM",
    "description": "Purchase at Abshire-Lowe",
    "amount": "+00000012.34",
    "orig_date": "2022-06-10",
    "proc_date": "2022-06-11",
    "merchant_id": "800000000",
    "merchant_name": "Abshire-Lowe",
    "merchant_city": "North Enoshaven",
    "merchant_zip": "72112"
}))]
pub struct TranAddFields {
    /// `TTYPCD`, the transaction type code, `LENGTH=2` (`COTRN02-021`).
    #[serde(default)]
    #[schema(max_length = 2)]
    #[validate(length(max = 2))]
    pub type_cd: String,
    /// `TCATCD`, the category code, four digits as typed, `LENGTH=4` (`COTRN02-024`).
    #[serde(default)]
    #[schema(max_length = 4)]
    #[validate(length(max = 4))]
    pub cat_cd: String,
    /// `TRNSRC`, the source, `LENGTH=10` (`COTRN02-027`).
    #[serde(default)]
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub source: String,
    /// `TDESC`, the description, `LENGTH=60` (`COTRN02-030`).
    #[serde(default)]
    #[schema(max_length = 60)]
    #[validate(length(max = 60))]
    pub description: String,
    /// `TRNAMT`, the amount as text in the hinted `-99999999.99` layout, which COTRN02C edits,
    /// `LENGTH=12` (`COTRN02-033`).
    #[serde(default)]
    #[schema(max_length = 12)]
    #[validate(length(max = 12))]
    pub amount: String,
    /// `TORIGDT`, the original date as typed, `YYYY-MM-DD`, raw input to COTRN02C's date edit,
    /// `LENGTH=10` (`COTRN02-036`).
    #[serde(default)]
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub orig_date: String,
    /// `TPROCDT`, the processing date as typed, raw input to COTRN02C's date edit, `LENGTH=10`
    /// (`COTRN02-039`).
    #[serde(default)]
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub proc_date: String,
    /// `MID`, the merchant ID, nine digits, `LENGTH=9` (`COTRN02-045`).
    #[serde(default)]
    #[schema(max_length = 9)]
    #[validate(length(max = 9))]
    pub merchant_id: String,
    /// `MNAME`, the merchant name, `LENGTH=30` (`COTRN02-048`).
    #[serde(default)]
    #[schema(max_length = 30)]
    #[validate(length(max = 30))]
    pub merchant_name: String,
    /// `MCITY`, the merchant city, `LENGTH=25` (`COTRN02-051`).
    #[serde(default)]
    #[schema(max_length = 25)]
    #[validate(length(max = 25))]
    pub merchant_city: String,
    /// `MZIP`, the merchant ZIP, `LENGTH=10` (`COTRN02-054`).
    #[serde(default)]
    #[schema(max_length = 10)]
    #[validate(length(max = 10))]
    pub merchant_zip: String,
}

/// The body of endpoint 13 (COTRN02C-060).
// The schema name `TranAddRequest` is fixed (Generated API Types Only). `Debug` shows a typed
// card number as sixteen `*`, and `Serialize` leaves it out, so neither a log line nor a
// serialised copy of the request can disclose it (Cardholder Data Redaction).
#[derive(Clone, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({
    "aid": "ENTER",
    "acct_id": "00000000050",
    "fields": {
        "type_cd": "01",
        "cat_cd": "0001",
        "source": "POS TERM",
        "description": "Purchase at Abshire-Lowe",
        "amount": "+00000012.34",
        "orig_date": "2022-06-10",
        "proc_date": "2022-06-11",
        "merchant_id": "800000000",
        "merchant_name": "Abshire-Lowe",
        "merchant_city": "North Enoshaven",
        "merchant_zip": "72112"
    },
    "confirm": "Y"
}))]
pub struct TranAddRequest {
    /// The attention key: `ENTER` or `PF5` (COTRN02C-007).
    pub aid: Aid,
    /// `ACTIDIN`, the account key as typed, `LENGTH=11` (`COTRN02-013`). A non-blank account
    /// decides the key, whatever `card_num` holds (COTRN02C-010, COTRN02C-011).
    #[schema(max_length = 11)]
    #[validate(length(max = 11))]
    pub acct_id: Option<String>,
    /// `CARDNIN`, the card key as typed, `LENGTH=16` (`COTRN02-017`); read only when `acct_id`
    /// is blank (COTRN02C-015, COTRN02C-016). Write-only: it is request input and never part of
    /// a response.
    // `skip_serializing_if` rather than `skip_serializing`: utoipa reads `skip_serializing` as
    // `skip` and would drop the member from the schema the client is generated from.
    #[serde(default, skip_serializing_if = "never_serialised")]
    #[schema(write_only, max_length = 16, example = "************5740")]
    #[validate(length(max = 16))]
    pub card_num: Option<String>,
    /// The eleven data fields.
    #[validate(nested)]
    pub fields: TranAddFields,
    /// `CONFIRM`, `Y`, `y`, `N`, `n` or blank as typed, `LENGTH=1` (`COTRN02-057`); any other
    /// value is COTRN02C's own edit (COTRN02C-040 to COTRN02C-042).
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub confirm: String,
}

/// The `skip_serializing_if` predicate of [`TranAddRequest::card_num`]: always true, so the
/// typed card number is never serialised.
const fn never_serialised<T>(_: &T) -> bool {
    true
}

/// A typed card number shows as sixteen `*`; every other member as derived.
impl fmt::Debug for TranAddRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranAddRequest")
            .field("aid", &self.aid)
            .field("acct_id", &self.acct_id)
            .field(
                "card_num",
                &self.card_num.as_ref().map(|_| CARD_INPUT_REDACTED),
            )
            .field("fields", &self.fields)
            .field("confirm", &self.confirm)
            .finish()
    }
}

/// The data fields the screen shows with the confirmation prompt, each as its map field holds
/// it (COTRN02C-057).
///
/// The text fields keep their trailing spaces, the amount is COTRN02C's `+99999999.99` edit and
/// the dates are typed, `None` for a blank date; the screen writes each date into its input as
/// `YYYY-MM-DD`, the ten characters the COBOL shows.
// The schema name `TranAddCopiedFields` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "type_cd": "01",
    "cat_cd": "0001",
    "source": "POS TERM  ",
    "description": "Purchase at Abshire-Lowe                                    ",
    "amount": "+00000012.34",
    "orig_date": "2022-06-10",
    "proc_date": "2022-06-11",
    "merchant_id": "800000000",
    "merchant_name": "Abshire-Lowe                  ",
    "merchant_city": "North Enoshaven          ",
    "merchant_zip": "72112     "
}))]
pub struct TranAddCopiedFields {
    /// `TTYPCD`, two characters.
    pub type_cd: String,
    /// `TCATCD`, four digits.
    pub cat_cd: String,
    /// `TRNSRC`, ten characters.
    pub source: String,
    /// `TDESC`, sixty characters.
    pub description: String,
    /// `TRNAMT`: a sign, eight digits, `.` and two digits (COTRN02C-036, COTRN02C-055).
    pub amount: String,
    /// `TORIGDT`, typed.
    pub orig_date: Option<NaiveDate>,
    /// `TPROCDT`, typed.
    pub proc_date: Option<NaiveDate>,
    /// `MID`, nine digits.
    pub merchant_id: String,
    /// `MNAME`, thirty characters.
    pub merchant_name: String,
    /// `MCITY`, twenty-five characters.
    pub merchant_city: String,
    /// `MZIP`, ten characters.
    pub merchant_zip: String,
}

/// A `PIC X(n)` map field as the screen holds it: every byte, the trailing spaces included
/// (COTRN02C-057). `super::text` drops them, so it is not used for these fields.
fn shown<const N: usize>(field: &PicX<N>) -> String {
    field.as_bytes().iter().copied().map(char::from).collect()
}

/// The fields once every COTRN02C edit has passed: entered on ENTER, copied on PF5
/// (COTRN02C-041, COTRN02C-056, COTRN02C-057).
impl From<&rules::ValidatedTran> for TranAddCopiedFields {
    fn from(fields: &rules::ValidatedTran) -> Self {
        Self {
            type_cd: shown(fields.type_cd()),
            cat_cd: shown(fields.cat_cd()),
            source: shown(fields.source()),
            description: shown(fields.description()),
            amount: fields.amount_text().to_owned(),
            orig_date: Some(fields.orig_date()),
            proc_date: Some(fields.proc_date()),
            merchant_id: shown(fields.merchant_id()),
            merchant_name: shown(fields.merchant_name()),
            merchant_city: shown(fields.merchant_city()),
            merchant_zip: shown(fields.merchant_zip()),
        }
    }
}

/// The PF5 copy of the transaction with the highest ID, before the edits run on it
/// (COTRN02C-054, COTRN02C-055).
impl From<rules::CopiedFields> for TranAddCopiedFields {
    fn from(copied: rules::CopiedFields) -> Self {
        Self {
            type_cd: copied.type_cd().to_owned(),
            cat_cd: copied.cat_cd().to_owned(),
            source: copied.source().to_owned(),
            description: copied.description().to_owned(),
            amount: copied.amount().to_owned(),
            orig_date: copied.orig_date(),
            proc_date: copied.proc_date(),
            merchant_id: copied.merchant_id().to_owned(),
            merchant_name: copied.merchant_name().to_owned(),
            merchant_city: copied.merchant_city().to_owned(),
            merchant_zip: copied.merchant_zip().to_owned(),
        }
    }
}

/// The 200 body of endpoint 13: what the COBOL screen shows after the confirmation prompt or
/// after the add (COTRN02C-041, COTRN02C-049, COTRN02C-057).
///
/// - **Prompt** (confirmation blank, `N` or `n`): the account and the card, entered or filled
///   in from the cross-reference, the card redacted; the data fields as edited, in `copied`;
///   field `confirm`; message `Confirm to add this transaction...`; no `new_tran_id`. Nothing
///   was written.
/// - **Added** (confirmation `Y` or `y`): every key and data field empty, field `acct_id`, the
///   success message and the new transaction ID.
// The schema name `TranAddResult` is fixed (Generated API Types Only). Beyond the key, the
// fields, the message and the new ID it carries `field`, the input the cursor goes to, which
// the prompt row names (`MOVE -1 TO CONFIRML`) and the screen focuses. It has no
// `Deserialize`: `CardNumber` cannot be read back from its redacted form.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[schema(example = json!({
    "acct_id": "00000000050",
    "card_num": "************5740",
    "copied": {
        "type_cd": "01",
        "cat_cd": "0001",
        "source": "POS TERM  ",
        "description": "Purchase at Abshire-Lowe                                    ",
        "amount": "+00000012.34",
        "orig_date": "2022-06-10",
        "proc_date": "2022-06-11",
        "merchant_id": "800000000",
        "merchant_name": "Abshire-Lowe                  ",
        "merchant_city": "North Enoshaven          ",
        "merchant_zip": "72112     "
    },
    "field": "confirm",
    "message": "Confirm to add this transaction...",
    "new_tran_id": null
}))]
pub struct TranAddResult {
    /// `ACTIDIN`: the 11 digits entered, or filled in from the cross-reference on the card path
    /// (COTRN02C-017); absent after the add.
    pub acct_id: Option<String>,
    /// `CARDNIN`: the card of the transaction, entered or filled in from the cross-reference on
    /// the account path, as twelve `*` and its last four digits (COTRN02C-012, `E32`); absent
    /// after the add.
    #[schema(value_type = Option<String>, example = "************5740")]
    pub card_num: Option<CardNumber>,
    /// The data fields the prompt shows: the request's on ENTER, the copied transaction's on
    /// PF5, each as COTRN02C edited it; absent after the add (COTRN02C-056, COTRN02C-057).
    pub copied: Option<TranAddCopiedFields>,
    /// The request member the cursor goes to: `confirm` after the prompt, `acct_id` after the
    /// add (COTRN02C-041, COTRN02C-049).
    pub field: String,
    /// The message row: `Confirm to add this transaction...`, or the success message with the
    /// new ID (COTRN02C-041, COTRN02C-049).
    pub message: String,
    /// The 16-digit ID of the inserted transaction, only after the add (COTRN02C-049).
    pub new_tran_id: Option<String>,
}

/// The wire name of the request member `field` names: the keys and `confirm` of
/// [`TranAddRequest`], every other a member of [`TranAddFields`], as the rejections of
/// COTRN02C's failures name them.
const fn request_member(field: rules::AddField) -> &'static str {
    use rules::AddField as F;
    match field {
        F::AcctId => "acct_id",
        F::CardNum => "card_num",
        F::TypeCd => "type_cd",
        F::CatCd => "cat_cd",
        F::Source => "source",
        F::Description => "description",
        F::Amount => "amount",
        F::OrigDate => "orig_date",
        F::ProcDate => "proc_date",
        F::MerchantId => "merchant_id",
        F::MerchantName => "merchant_name",
        F::MerchantCity => "merchant_city",
        F::MerchantZip => "merchant_zip",
        F::Confirm => "confirm",
    }
}

/// The service's outcome as the response carries it; every text is the domain's.
impl From<rules::AddOutcome> for TranAddResult {
    fn from(outcome: rules::AddOutcome) -> Self {
        let message = outcome.message();
        let field = request_member(outcome.focus()).to_owned();
        match outcome {
            rules::AddOutcome::Prompt(prompt) => Self {
                acct_id: Some(digits(prompt.key().acct_id())),
                card_num: Some(prompt.key().card_num().clone()),
                copied: Some(TranAddCopiedFields::from(prompt.fields())),
                field,
                message,
                new_tran_id: None,
            },
            rules::AddOutcome::Added(new_id) => Self {
                acct_id: None,
                card_num: None,
                copied: None,
                field,
                message,
                new_tran_id: Some(text(&new_id)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use chrono::NaiveDate;
    use domain::copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
    use domain::money::{Decimal, Money};
    use domain::online::cotrn02c as rules;
    use domain::online::csutldtc::{LeFeedback, LeResult};
    use domain::records::{CardXrefRecord, TransactionRecord};
    use domain::sensitive::CardNumber;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};

    use super::{
        Aid, TranAddCopiedFields, TranAddFields, TranAddRequest, TranAddResult, request_member,
    };
    use crate::dto::{FIELD_LENGTH_MESSAGE, INVALID_BODY, ValidJson};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The status, message and field of a rejection.
    type Rejection = (StatusCode, String, Option<String>);

    /// A typed card number no response, `Debug` or serialisation may echo.
    const CARD: &str = "4111111111115740";

    /// Its redacted form.
    const CARD_REDACTED: &str = "************5740";

    /// A request body whose every input is at its map `LENGTH`.
    fn full_body() -> Value {
        json!({
            "aid": "ENTER",
            "acct_id": "00000000050",
            "card_num": CARD,
            "fields": {
                "type_cd": "01",
                "cat_cd": "0001",
                "source": "S".repeat(10),
                "description": "D".repeat(60),
                "amount": "+00000012.34",
                "orig_date": "2022-06-10",
                "proc_date": "2022-06-11",
                "merchant_id": "800000000",
                "merchant_name": "N".repeat(30),
                "merchant_city": "C".repeat(25),
                "merchant_zip": "Z".repeat(10)
            },
            "confirm": "Y"
        })
    }

    /// `body` through `ValidJson`, the extractor of endpoint 13.
    async fn extract(body: &Value) -> Result<Result<TranAddRequest, Rejection>, Box<dyn StdError>> {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/transactions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))?;
        Ok(ValidJson::<TranAddRequest>::from_request(request, &())
            .await
            .map(|ValidJson(request)| request)
            .map_err(|error| {
                (
                    error.status(),
                    error.message().to_owned(),
                    error.field().map(str::to_owned),
                )
            }))
    }

    /// The schema of `T` as JSON.
    fn schema_json<T: PartialSchema>() -> Result<Value, serde_json::Error> {
        serde_json::to_value(T::schema())
    }

    /// The data fields every edit passes, with the stand-in date verdict that accepts them.
    fn validated() -> Result<rules::ValidatedTran, rules::AddFailure> {
        let input = rules::TranAddInput {
            type_cd: "01",
            cat_cd: "0001",
            source: "POS TERM",
            description: "Purchase at Abshire-Lowe",
            amount: "+00000012.34",
            orig_date: "2022-06-10",
            proc_date: "2022-06-11",
            merchant_id: "800000000",
            merchant_name: "Abshire-Lowe",
            merchant_city: "North Enoshaven",
            merchant_zip: "72112",
            confirm: "",
        };
        rules::edit_data_with(&input, |date, mask| {
            LeResult::new(date, mask, LeFeedback::Valid)
        })
    }

    /// The key the account path fills in from a cross-reference holding [`CARD`].
    fn filled_key() -> Result<rules::FilledKey, Box<dyn StdError>> {
        let xref = CardXrefRecord {
            card_num: CardNumber::new(PicX::from_text(CARD)?),
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::spaces(),
        };
        let request = rules::edit_keys(Some("00000000050"), None)?;
        Ok(rules::fill_key(request, rules::XrefLookup::Found(&xref))?)
    }

    /// `text` space-padded on the right to `width` characters.
    fn padded(text: &str, width: usize) -> String {
        format!("{text:<width$}")
    }

    #[test]
    fn aid_travels_as_enter_and_pf5_and_maps_onto_the_program_key() -> TestResult {
        for (aid, wire, program) in [
            (Aid::Enter, "\"ENTER\"", rules::Aid::Enter),
            (Aid::Pf5, "\"PF5\"", rules::Aid::Pf5),
        ] {
            assert_eq!(serde_json::to_string(&aid)?, wire);
            assert_eq!(serde_json::from_str::<Aid>(wire)?, aid);
            assert_eq!(rules::Aid::from(aid), program);
        }
        for wire in ["\"Enter\"", "\"pf5\"", "\"PF4\"", "\"PF3\"", "\"\"", "5"] {
            assert!(serde_json::from_str::<Aid>(wire).is_err(), "{wire}");
        }
        assert_eq!(Aid::name(), "Cotrn02cAid");
        assert_eq!(
            schema_json::<Aid>()?.get("enum"),
            Some(&json!(["ENTER", "PF5"]))
        );
        Ok(())
    }

    #[test]
    fn absent_members_are_empty_text() -> TestResult {
        let request: TranAddRequest =
            serde_json::from_value(json!({"aid": "PF5", "acct_id": "00000000050", "fields": {}}))?;
        assert_eq!(request.aid, Aid::Pf5);
        assert_eq!(request.acct_id.as_deref(), Some("00000000050"));
        assert_eq!(request.card_num, None);
        assert_eq!(request.fields, TranAddFields::default());
        assert_eq!(request.confirm, "");
        let request: TranAddRequest = serde_json::from_value(
            json!({"aid": "ENTER", "acct_id": null, "card_num": CARD, "fields": {}}),
        )?;
        assert_eq!(request.acct_id, None);
        assert_eq!(request.card_num.as_deref(), Some(CARD));
        // `fields` is the one member a body cannot leave out.
        assert!(serde_json::from_value::<TranAddRequest>(json!({"aid": "ENTER"})).is_err());
        Ok(())
    }

    // audit: COTRN02-013
    // audit: COTRN02-017
    // audit: COTRN02-021
    // audit: COTRN02-024
    // audit: COTRN02-027
    // audit: COTRN02-030
    // audit: COTRN02-033
    // audit: COTRN02-036
    // audit: COTRN02-039
    // audit: COTRN02-045
    // audit: COTRN02-048
    // audit: COTRN02-051
    // audit: COTRN02-054
    // audit: COTRN02-057
    #[tokio::test]
    async fn each_input_is_held_to_its_map_length() -> TestResult {
        let request = extract(&full_body())
            .await?
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(request.fields.description.len(), 60);

        for (pointer, width, field) in [
            ("/acct_id", 11, "acct_id"),
            ("/card_num", 16, "card_num"),
            ("/fields/type_cd", 2, "fields.type_cd"),
            ("/fields/cat_cd", 4, "fields.cat_cd"),
            ("/fields/source", 10, "fields.source"),
            ("/fields/description", 60, "fields.description"),
            ("/fields/amount", 12, "fields.amount"),
            ("/fields/orig_date", 10, "fields.orig_date"),
            ("/fields/proc_date", 10, "fields.proc_date"),
            ("/fields/merchant_id", 9, "fields.merchant_id"),
            ("/fields/merchant_name", 30, "fields.merchant_name"),
            ("/fields/merchant_city", 25, "fields.merchant_city"),
            ("/fields/merchant_zip", 10, "fields.merchant_zip"),
            ("/confirm", 1, "confirm"),
        ] {
            let mut body = full_body();
            *body.pointer_mut(pointer).ok_or(pointer)? = json!("9".repeat(width + 1));
            let error = extract(&body).await?.err().ok_or(pointer)?;
            assert_eq!(
                error,
                (
                    StatusCode::BAD_REQUEST,
                    FIELD_LENGTH_MESSAGE.to_owned(),
                    Some(field.to_owned())
                ),
                "{pointer}"
            );
            assert!(!error.1.contains(CARD));
        }
        Ok(())
    }

    #[tokio::test]
    async fn an_unserved_key_is_refused_before_any_edit() -> TestResult {
        for aid in ["PF4", "PF7", "enter", ""] {
            let mut body = full_body();
            *body.pointer_mut("/aid").ok_or("aid")? = json!(aid);
            let error = extract(&body).await?.err().ok_or(aid)?;
            assert_eq!(
                error,
                (StatusCode::BAD_REQUEST, INVALID_BODY.to_owned(), None),
                "{aid}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_typed_card_number_is_never_echoed() -> TestResult {
        let request: TranAddRequest = serde_json::from_value(full_body())?;
        let debug = format!("{request:?}");
        assert!(!debug.contains(CARD) && !debug.contains("5740"), "{debug}");
        assert!(
            debug.contains("card_num: Some(\"****************\")"),
            "{debug}"
        );
        assert!(debug.contains("acct_id: Some(\"00000000050\")"), "{debug}");

        let serialised = serde_json::to_value(&request)?;
        assert_eq!(serialised.get("card_num"), None);
        assert_eq!(serialised.get("acct_id"), Some(&json!("00000000050")));
        assert!(!serialised.to_string().contains(CARD));

        let unkeyed = TranAddRequest {
            card_num: None,
            ..request
        };
        assert!(format!("{unkeyed:?}").contains("card_num: None"));
        Ok(())
    }

    #[test]
    fn the_schemas_keep_the_card_number_write_only_and_redacted() -> TestResult {
        let request = schema_json::<TranAddRequest>()?;
        let card = request.pointer("/properties/card_num").ok_or("card_num")?;
        assert_eq!(card.get("writeOnly"), Some(&json!(true)));
        assert_eq!(card.get("example"), Some(&json!(CARD_REDACTED)));
        assert_eq!(card.get("maxLength"), Some(&json!(16)));
        let fields = schema_json::<TranAddFields>()?;
        assert_eq!(
            fields.pointer("/properties/description/maxLength"),
            Some(&json!(60))
        );
        let required = request.get("required").ok_or("required")?;
        assert_eq!(required, &json!(["aid", "fields"]));

        let result = schema_json::<TranAddResult>()?;
        assert_eq!(
            result.pointer("/properties/card_num/example"),
            Some(&json!(CARD_REDACTED))
        );
        let copied = schema_json::<TranAddCopiedFields>()?;
        assert_eq!(
            copied.pointer("/properties/orig_date/format"),
            Some(&json!("date"))
        );
        for (name, expected) in [
            (TranAddFields::name(), "TranAddFields"),
            (TranAddRequest::name(), "TranAddRequest"),
            (TranAddCopiedFields::name(), "TranAddCopiedFields"),
            (TranAddResult::name(), "TranAddResult"),
        ] {
            assert_eq!(name, expected);
        }
        let all = [request, result, copied, schema_json::<TranAddFields>()?].map(|s| s.to_string());
        assert!(all.iter().all(|schema| !schema.contains(CARD)));
        Ok(())
    }

    #[test]
    fn every_program_field_names_a_request_member() -> TestResult {
        use rules::AddField as F;
        let request = schema_json::<TranAddRequest>()?;
        let fields = schema_json::<TranAddFields>()?;
        for field in [
            F::AcctId,
            F::CardNum,
            F::TypeCd,
            F::CatCd,
            F::Source,
            F::Description,
            F::Amount,
            F::OrigDate,
            F::ProcDate,
            F::MerchantId,
            F::MerchantName,
            F::MerchantCity,
            F::MerchantZip,
            F::Confirm,
        ] {
            let name = request_member(field);
            let top = request.pointer(&format!("/properties/{name}")).is_some();
            let nested = fields.pointer(&format!("/properties/{name}")).is_some();
            assert!(top != nested, "{field:?} -> {name}");
        }
        Ok(())
    }

    // audit: COTRN02C-041
    // audit: COTRN02C-057
    #[test]
    fn the_prompt_carries_the_key_the_edited_fields_and_the_focus() -> TestResult {
        let outcome = rules::AddOutcome::prompt(filled_key()?, validated()?, None);
        let result = TranAddResult::from(outcome);
        let body = serde_json::to_value(&result)?;
        assert_eq!(
            body,
            json!({
                "acct_id": "00000000050",
                "card_num": CARD_REDACTED,
                "copied": {
                    "type_cd": "01",
                    "cat_cd": "0001",
                    "source": padded("POS TERM", 10),
                    "description": padded("Purchase at Abshire-Lowe", 60),
                    "amount": "+00000012.34",
                    "orig_date": "2022-06-10",
                    "proc_date": "2022-06-11",
                    "merchant_id": "800000000",
                    "merchant_name": padded("Abshire-Lowe", 30),
                    "merchant_city": padded("North Enoshaven", 25),
                    "merchant_zip": padded("72112", 10)
                },
                "field": "confirm",
                "message": "Confirm to add this transaction...",
                "new_tran_id": null
            })
        );
        assert!(!body.to_string().contains(CARD));
        assert!(!format!("{result:?}").contains(CARD));
        Ok(())
    }

    // audit: COTRN02C-049
    #[test]
    fn the_add_empties_every_field_and_reports_the_new_id() -> TestResult {
        let new_id = PicX::from_text("0000000000000301")?;
        let result = TranAddResult::from(rules::AddOutcome::added(new_id.clone()));
        assert_eq!(
            serde_json::to_value(&result)?,
            json!({
                "acct_id": null,
                "card_num": null,
                "copied": null,
                "field": "acct_id",
                "message": rules::success_message(&new_id),
                "new_tran_id": "0000000000000301"
            })
        );
        assert_eq!(
            result.message,
            "Transaction added successfully.  Your Tran ID is 0000000000000301."
        );
        Ok(())
    }

    // audit: COTRN02C-054
    // audit: COTRN02C-055
    #[test]
    fn the_pf5_copy_keeps_each_map_field_and_types_its_dates() -> TestResult {
        let day = NaiveDate::from_ymd_opt(2022, 6, 10).ok_or("date")?;
        let last = TransactionRecord {
            id: PicX::from_text("0000000000000300")?,
            type_cd: PicX::from_text("02")?,
            cat_cd: PicU::new(5)?,
            source: PicX::from_text("OPERATOR")?,
            desc: PicX::from_text(&"R".repeat(100))?,
            amt: Money::<9>::new(Decimal::new(-123_456, 2))?,
            merchant_id: PicU::new(42)?,
            merchant_name: PicX::from_text(&"M".repeat(50))?,
            merchant_city: PicX::from_text("Springfield")?,
            merchant_zip: PicX::from_text("12345")?,
            card_num: CardNumber::new(PicX::from_text(CARD)?),
            orig_ts: PicTimestamp::new(Some(CobolTimestamp::new(
                day.and_hms_micro_opt(10, 0, 0, 0).ok_or("time")?,
                TsForm::Iso,
            )?)),
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        };
        let copied = TranAddCopiedFields::from(rules::copy_last(&last));
        assert_eq!(
            copied,
            TranAddCopiedFields {
                type_cd: "02".to_owned(),
                cat_cd: "0005".to_owned(),
                source: padded("OPERATOR", 10),
                description: "R".repeat(60),
                amount: "-00001234.56".to_owned(),
                orig_date: Some(day),
                proc_date: None,
                merchant_id: "000000042".to_owned(),
                merchant_name: "M".repeat(30),
                merchant_city: padded("Springfield", 25),
                merchant_zip: padded("12345", 10),
            }
        );
        let round_trip: TranAddCopiedFields =
            serde_json::from_value(serde_json::to_value(&copied)?)?;
        assert_eq!(round_trip, copied);
        Ok(())
    }
}
