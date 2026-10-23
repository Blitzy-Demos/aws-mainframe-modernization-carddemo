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
//! [`Aid`] serves `ENTER` and `PF5`, the only keys its schema names. Any other key name a body
//! carries is read as the hidden [`Aid::Unserved`], so that it gets the program's own answer
//! rather than the rejection of an unreadable body: its conversion into the program's key is
//! COTRN02C's `AddFailure::invalid_key`, which the service returns before anything is read or
//! written, 400 with the 50-byte `Invalid key pressed. Please see below...` and no field
//! (COTRN02C-007). An `aid` that is missing or is not text at all still leaves the body
//! unreadable, and `ValidJson` answers it with its own 400.

use core::fmt;

use chrono::NaiveDate;
use domain::copybook::PicX;
use domain::online::cotrn02c as rules;
use domain::sensitive::CardNumber;
use serde::de::{self, DeserializeSeed, EnumAccess, IgnoredAny, MapAccess, VariantAccess, Visitor};
use serde::ser::{self, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::ToSchema;
use validator::Validate;

use super::{digits, text};

/// What the `Debug` of a [`TranAddRequest`] prints for a typed card number: sixteen `*`. The
/// input need not be a card number at all, so not even its last four characters are shown.
const CARD_INPUT_REDACTED: &str = "****************";

/// A member or variant name of this module's types, read as the entry of its list that it
/// equals, or `None` for a name the list does not hold.
#[derive(Clone, Copy)]
struct WireName(&'static [&'static str]);

impl<'de> DeserializeSeed<'de> for WireName {
    type Value = Option<&'static str>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_identifier(self)
    }
}

impl Visitor<'_> for WireName {
    type Value = Option<&'static str>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a member or variant name")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
        Ok(self.0.iter().copied().find(|wire| *wire == name))
    }
}

/// Reads the value of `name` into `slot`, refusing a member the object repeats.
fn member<'de, A, T>(map: &mut A, slot: &mut Option<T>, name: &'static str) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

/// A member whose schema is an object, read through [`super::object`].
struct Object<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        super::object(deserializer).map(Self)
    }
}

/// The attention key of an endpoint 13 request, `TranAddRequest.aid` (COTRN02C-007).
///
/// `ENTER` and `PF5` are served. Any other key name is accepted, so that it gets the program's
/// own answer, 400 `Invalid key pressed. Please see below...` with nothing read or written, never
/// the rejection of an unreadable body (COTRN02C-006, COTRN02C-007). F3 and F4 are the screen's,
/// which serves them without a request (COTRN02C-004, COTRN02C-005).
// The schema name `Cotrn02cAid` is fixed (Generated API Types Only): each program declares its
// own `Aid`, so the bare name would collide in `components.schemas`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ToSchema)]
#[schema(as = Cotrn02cAid, rename_all = "UPPERCASE")]
pub enum Aid {
    /// `ENTER`, `PROCESS-ENTER-KEY`: the key edits, the data edits on the request's fields, then
    /// the confirmation (COTRN02C-009 to COTRN02C-042).
    Enter,
    /// `PF5`, `COPY-LAST-TRAN-DATA`: the key edits, the copy of the data fields of the
    /// transaction with the highest ID, then ENTER on the copied fields (COTRN02C-052 to
    /// COTRN02C-056).
    Pf5,
    /// Any other key name. Absent from the schema and never serialised: the screen never sends
    /// it, and serialising it is an error rather than a value the server would read differently.
    // utoipa's `ToSchema` reads serde's skip attributes and has no attribute of its own that
    // leaves a unit variant out of a plain enum's schema (its variant options are `rename`
    // alone), so this serde attribute stays as the schema's input; no serde derive reads it, and
    // the hand-written `Serialize` below refuses this variant just as the attribute states.
    #[serde(skip_serializing)]
    Unserved,
}

/// `ENTER` and `PF5` by name; an unserved key is an error, never a value.
impl Serialize for Aid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Enter => serializer.serialize_unit_variant("Aid", 0, "ENTER"),
            Self::Pf5 => serializer.serialize_unit_variant("Aid", 1, "PF5"),
            Self::Unserved => Err(ser::Error::custom("an unserved key is never serialised")),
        }
    }
}

/// The key names [`Aid`] serves, in declaration order.
const AID_NAMES: &[&str] = &["ENTER", "PF5"];

/// `ENTER` and `PF5` by name, and any other text, the empty text included, as
/// [`Aid::Unserved`]; a value that is not text is refused.
impl<'de> Deserialize<'de> for Aid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_enum("Aid", AID_NAMES, AidVisitor)
    }
}

/// The visitor of an [`Aid`] key name.
struct AidVisitor;

impl<'de> Visitor<'de> for AidVisitor {
    type Value = Aid;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("enum Aid")
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Aid, A::Error> {
        let (name, variant) = data.variant_seed(WireName(AID_NAMES))?;
        variant.unit_variant()?;
        Ok(match name {
            Some("ENTER") => Aid::Enter,
            Some("PF5") => Aid::Pf5,
            _ => Aid::Unserved,
        })
    }
}

/// The key as `domain::online::cotrn02c` names it. An unserved key has none: it is the
/// program's `WHEN OTHER` branch, answered with [`rules::AddFailure::invalid_key`]
/// (COTRN02C-007).
impl TryFrom<Aid> for rules::Aid {
    type Error = rules::AddFailure;

    fn try_from(aid: Aid) -> Result<Self, Self::Error> {
        match aid {
            Aid::Enter => Ok(Self::Enter),
            Aid::Pf5 => Ok(Self::Pf5),
            Aid::Unserved => Err(rules::AddFailure::invalid_key()),
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
#[derive(Debug, Clone, Default, PartialEq, Eq, ToSchema, Validate)]
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
    #[schema(required = false, max_length = 2)]
    #[validate(length(max = 2))]
    pub type_cd: String,
    /// `TCATCD`, the category code, four digits as typed, `LENGTH=4` (`COTRN02-024`).
    #[schema(required = false, max_length = 4)]
    #[validate(length(max = 4))]
    pub cat_cd: String,
    /// `TRNSRC`, the source, `LENGTH=10` (`COTRN02-027`).
    #[schema(required = false, max_length = 10)]
    #[validate(length(max = 10))]
    pub source: String,
    /// `TDESC`, the description, `LENGTH=60` (`COTRN02-030`).
    #[schema(required = false, max_length = 60)]
    #[validate(length(max = 60))]
    pub description: String,
    /// `TRNAMT`, the amount as text in the hinted `-99999999.99` layout, which COTRN02C edits,
    /// `LENGTH=12` (`COTRN02-033`).
    #[schema(required = false, max_length = 12)]
    #[validate(length(max = 12))]
    pub amount: String,
    /// `TORIGDT`, the original date as typed, `YYYY-MM-DD`, raw input to COTRN02C's date edit,
    /// `LENGTH=10` (`COTRN02-036`).
    #[schema(required = false, max_length = 10)]
    #[validate(length(max = 10))]
    pub orig_date: String,
    /// `TPROCDT`, the processing date as typed, raw input to COTRN02C's date edit, `LENGTH=10`
    /// (`COTRN02-039`).
    #[schema(required = false, max_length = 10)]
    #[validate(length(max = 10))]
    pub proc_date: String,
    /// `MID`, the merchant ID, nine digits, `LENGTH=9` (`COTRN02-045`).
    #[schema(required = false, max_length = 9)]
    #[validate(length(max = 9))]
    pub merchant_id: String,
    /// `MNAME`, the merchant name, `LENGTH=30` (`COTRN02-048`).
    #[schema(required = false, max_length = 30)]
    #[validate(length(max = 30))]
    pub merchant_name: String,
    /// `MCITY`, the merchant city, `LENGTH=25` (`COTRN02-051`).
    #[schema(required = false, max_length = 25)]
    #[validate(length(max = 25))]
    pub merchant_city: String,
    /// `MZIP`, the merchant ZIP, `LENGTH=10` (`COTRN02-054`).
    #[schema(required = false, max_length = 10)]
    #[validate(length(max = 10))]
    pub merchant_zip: String,
}

/// Every member by name, in declaration order.
impl Serialize for TranAddFields {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("TranAddFields", 11)?;
        out.serialize_field("type_cd", &self.type_cd)?;
        out.serialize_field("cat_cd", &self.cat_cd)?;
        out.serialize_field("source", &self.source)?;
        out.serialize_field("description", &self.description)?;
        out.serialize_field("amount", &self.amount)?;
        out.serialize_field("orig_date", &self.orig_date)?;
        out.serialize_field("proc_date", &self.proc_date)?;
        out.serialize_field("merchant_id", &self.merchant_id)?;
        out.serialize_field("merchant_name", &self.merchant_name)?;
        out.serialize_field("merchant_city", &self.merchant_city)?;
        out.serialize_field("merchant_zip", &self.merchant_zip)?;
        out.end()
    }
}

/// The members of [`TranAddFields`], in declaration order.
const TRAN_ADD_FIELDS_MEMBERS: &[&str] = &[
    "type_cd",
    "cat_cd",
    "source",
    "description",
    "amount",
    "orig_date",
    "proc_date",
    "merchant_id",
    "merchant_name",
    "merchant_city",
    "merchant_zip",
];

/// An absent member is the empty text; a repeated member is refused and an unknown one skipped.
impl<'de> Deserialize<'de> for TranAddFields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "TranAddFields",
            TRAN_ADD_FIELDS_MEMBERS,
            TranAddFieldsVisitor,
        )
    }
}

/// The visitor of [`TranAddFields`]'s members.
struct TranAddFieldsVisitor;

impl<'de> Visitor<'de> for TranAddFieldsVisitor {
    type Value = TranAddFields;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct TranAddFields")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TranAddFields, A::Error> {
        let mut type_cd = None;
        let mut cat_cd = None;
        let mut source = None;
        let mut description = None;
        let mut amount = None;
        let mut orig_date = None;
        let mut proc_date = None;
        let mut merchant_id = None;
        let mut merchant_name = None;
        let mut merchant_city = None;
        let mut merchant_zip = None;
        while let Some(name) = map.next_key_seed(WireName(TRAN_ADD_FIELDS_MEMBERS))? {
            match name {
                Some("type_cd") => member(&mut map, &mut type_cd, "type_cd")?,
                Some("cat_cd") => member(&mut map, &mut cat_cd, "cat_cd")?,
                Some("source") => member(&mut map, &mut source, "source")?,
                Some("description") => member(&mut map, &mut description, "description")?,
                Some("amount") => member(&mut map, &mut amount, "amount")?,
                Some("orig_date") => member(&mut map, &mut orig_date, "orig_date")?,
                Some("proc_date") => member(&mut map, &mut proc_date, "proc_date")?,
                Some("merchant_id") => member(&mut map, &mut merchant_id, "merchant_id")?,
                Some("merchant_name") => member(&mut map, &mut merchant_name, "merchant_name")?,
                Some("merchant_city") => member(&mut map, &mut merchant_city, "merchant_city")?,
                Some("merchant_zip") => member(&mut map, &mut merchant_zip, "merchant_zip")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(TranAddFields {
            type_cd: type_cd.unwrap_or_default(),
            cat_cd: cat_cd.unwrap_or_default(),
            source: source.unwrap_or_default(),
            description: description.unwrap_or_default(),
            amount: amount.unwrap_or_default(),
            orig_date: orig_date.unwrap_or_default(),
            proc_date: proc_date.unwrap_or_default(),
            merchant_id: merchant_id.unwrap_or_default(),
            merchant_name: merchant_name.unwrap_or_default(),
            merchant_city: merchant_city.unwrap_or_default(),
            merchant_zip: merchant_zip.unwrap_or_default(),
        })
    }
}

/// The body of endpoint 13 (COTRN02C-060).
// The schema name `TranAddRequest` is fixed (Generated API Types Only). `Debug` shows a typed
// card number as sixteen `*`, and `Serialize` leaves it out, so neither a log line nor a
// serialised copy of the request can disclose it (Cardholder Data Redaction).
#[derive(Clone, ToSchema, Validate)]
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
    /// The attention key: `ENTER` or `PF5`. Any other key name is answered with 400 `Invalid key
    /// pressed. Please see below...` and no field, before anything is read (COTRN02C-007).
    pub aid: Aid,
    /// `ACTIDIN`, the account key as typed, `LENGTH=11` (`COTRN02-013`). A non-blank account
    /// decides the key, whatever `card_num` holds (COTRN02C-010, COTRN02C-011).
    #[schema(max_length = 11)]
    #[validate(length(max = 11))]
    pub acct_id: Option<String>,
    /// `CARDNIN`, the card key as typed, `LENGTH=16` (`COTRN02-017`); read only when `acct_id`
    /// is blank (COTRN02C-015, COTRN02C-016). Write-only: it is request input and never part of
    /// a response.
    // The hand-written `Serialize` below leaves this member out, so a serialised request never
    // holds it, while the schema the client is generated from still lists it as write-only.
    #[schema(write_only, max_length = 16, example = "************5740")]
    #[validate(length(max = 16))]
    pub card_num: Option<String>,
    /// The eleven data fields.
    #[validate(nested)]
    pub fields: TranAddFields,
    /// `CONFIRM`, `Y`, `y`, `N`, `n` or blank as typed, `LENGTH=1` (`COTRN02-057`); any other
    /// value is COTRN02C's own edit (COTRN02C-040 to COTRN02C-042).
    #[schema(required = false, max_length = 1)]
    #[validate(length(max = 1))]
    pub confirm: String,
}

/// `aid`, `acct_id`, `fields` and `confirm` by name, in declaration order; the typed `card_num`
/// is left out.
impl Serialize for TranAddRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("TranAddRequest", 4)?;
        out.serialize_field("aid", &self.aid)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("fields", &self.fields)?;
        out.serialize_field("confirm", &self.confirm)?;
        out.end()
    }
}

/// The members of [`TranAddRequest`], in declaration order.
const TRAN_ADD_REQUEST_MEMBERS: &[&str] = &["aid", "acct_id", "card_num", "fields", "confirm"];

/// `aid` and `fields` are required, `fields` an object read through `super::object`; an
/// absent or `null` key is `None` and an absent `confirm` the empty text. A repeated member is
/// refused and an unknown one skipped.
impl<'de> Deserialize<'de> for TranAddRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "TranAddRequest",
            TRAN_ADD_REQUEST_MEMBERS,
            TranAddRequestVisitor,
        )
    }
}

/// The visitor of [`TranAddRequest`]'s members.
struct TranAddRequestVisitor;

impl<'de> Visitor<'de> for TranAddRequestVisitor {
    type Value = TranAddRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct TranAddRequest")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TranAddRequest, A::Error> {
        let mut aid = None;
        let mut acct_id = None;
        let mut card_num = None;
        let mut fields = None;
        let mut confirm = None;
        while let Some(name) = map.next_key_seed(WireName(TRAN_ADD_REQUEST_MEMBERS))? {
            match name {
                Some("aid") => member(&mut map, &mut aid, "aid")?,
                Some("acct_id") => member(&mut map, &mut acct_id, "acct_id")?,
                Some("card_num") => member(&mut map, &mut card_num, "card_num")?,
                Some("fields") => member(&mut map, &mut fields, "fields")?,
                Some("confirm") => member(&mut map, &mut confirm, "confirm")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(TranAddRequest {
            aid: aid.ok_or_else(|| de::Error::missing_field("aid"))?,
            acct_id: acct_id.flatten(),
            card_num: card_num.flatten(),
            fields: fields
                .map(|Object(fields)| fields)
                .ok_or_else(|| de::Error::missing_field("fields"))?,
            confirm: confirm.unwrap_or_default(),
        })
    }
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
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
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

/// Every member by name, in declaration order; a blank date is `null`.
impl Serialize for TranAddCopiedFields {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("TranAddCopiedFields", 11)?;
        out.serialize_field("type_cd", &self.type_cd)?;
        out.serialize_field("cat_cd", &self.cat_cd)?;
        out.serialize_field("source", &self.source)?;
        out.serialize_field("description", &self.description)?;
        out.serialize_field("amount", &self.amount)?;
        out.serialize_field("orig_date", &self.orig_date)?;
        out.serialize_field("proc_date", &self.proc_date)?;
        out.serialize_field("merchant_id", &self.merchant_id)?;
        out.serialize_field("merchant_name", &self.merchant_name)?;
        out.serialize_field("merchant_city", &self.merchant_city)?;
        out.serialize_field("merchant_zip", &self.merchant_zip)?;
        out.end()
    }
}

/// The members of [`TranAddCopiedFields`], in declaration order.
const TRAN_ADD_COPIED_FIELDS_MEMBERS: &[&str] = &[
    "type_cd",
    "cat_cd",
    "source",
    "description",
    "amount",
    "orig_date",
    "proc_date",
    "merchant_id",
    "merchant_name",
    "merchant_city",
    "merchant_zip",
];

/// Every text member is required; an absent or `null` date is `None`. A repeated member is
/// refused and an unknown one skipped.
impl<'de> Deserialize<'de> for TranAddCopiedFields {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "TranAddCopiedFields",
            TRAN_ADD_COPIED_FIELDS_MEMBERS,
            TranAddCopiedFieldsVisitor,
        )
    }
}

/// The visitor of [`TranAddCopiedFields`]'s members.
struct TranAddCopiedFieldsVisitor;

impl<'de> Visitor<'de> for TranAddCopiedFieldsVisitor {
    type Value = TranAddCopiedFields;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct TranAddCopiedFields")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TranAddCopiedFields, A::Error> {
        let mut type_cd = None;
        let mut cat_cd = None;
        let mut source = None;
        let mut description = None;
        let mut amount = None;
        let mut orig_date = None;
        let mut proc_date = None;
        let mut merchant_id = None;
        let mut merchant_name = None;
        let mut merchant_city = None;
        let mut merchant_zip = None;
        while let Some(name) = map.next_key_seed(WireName(TRAN_ADD_COPIED_FIELDS_MEMBERS))? {
            match name {
                Some("type_cd") => member(&mut map, &mut type_cd, "type_cd")?,
                Some("cat_cd") => member(&mut map, &mut cat_cd, "cat_cd")?,
                Some("source") => member(&mut map, &mut source, "source")?,
                Some("description") => member(&mut map, &mut description, "description")?,
                Some("amount") => member(&mut map, &mut amount, "amount")?,
                Some("orig_date") => member(&mut map, &mut orig_date, "orig_date")?,
                Some("proc_date") => member(&mut map, &mut proc_date, "proc_date")?,
                Some("merchant_id") => member(&mut map, &mut merchant_id, "merchant_id")?,
                Some("merchant_name") => member(&mut map, &mut merchant_name, "merchant_name")?,
                Some("merchant_city") => member(&mut map, &mut merchant_city, "merchant_city")?,
                Some("merchant_zip") => member(&mut map, &mut merchant_zip, "merchant_zip")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(TranAddCopiedFields {
            type_cd: type_cd.ok_or_else(|| de::Error::missing_field("type_cd"))?,
            cat_cd: cat_cd.ok_or_else(|| de::Error::missing_field("cat_cd"))?,
            source: source.ok_or_else(|| de::Error::missing_field("source"))?,
            description: description.ok_or_else(|| de::Error::missing_field("description"))?,
            amount: amount.ok_or_else(|| de::Error::missing_field("amount"))?,
            orig_date: orig_date.flatten(),
            proc_date: proc_date.flatten(),
            merchant_id: merchant_id.ok_or_else(|| de::Error::missing_field("merchant_id"))?,
            merchant_name: merchant_name
                .ok_or_else(|| de::Error::missing_field("merchant_name"))?,
            merchant_city: merchant_city
                .ok_or_else(|| de::Error::missing_field("merchant_city"))?,
            merchant_zip: merchant_zip.ok_or_else(|| de::Error::missing_field("merchant_zip"))?,
        })
    }
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
#[derive(Debug, Clone, ToSchema)]
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

/// Every member by name, in declaration order, an absent one as `null`; the card through
/// [`CardNumber`]'s own redacted serialisation.
impl Serialize for TranAddResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("TranAddResult", 6)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("card_num", &self.card_num)?;
        out.serialize_field("copied", &self.copied)?;
        out.serialize_field("field", &self.field)?;
        out.serialize_field("message", &self.message)?;
        out.serialize_field("new_tran_id", &self.new_tran_id)?;
        out.end()
    }
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
    use domain::messages::CCDA_MSG_INVALID_KEY;
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
    use crate::error::ScreenError;

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
        extract_text(body.to_string()).await
    }

    /// The JSON text `body`, well formed or not, through `ValidJson`.
    async fn extract_text(
        body: String,
    ) -> Result<Result<TranAddRequest, Rejection>, Box<dyn StdError>> {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/transactions")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))?;
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

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    #[test]
    fn aid_travels_as_enter_and_pf5_and_maps_onto_the_program_key() -> TestResult {
        for (aid, wire, program) in [
            (Aid::Enter, "\"ENTER\"", rules::Aid::Enter),
            (Aid::Pf5, "\"PF5\"", rules::Aid::Pf5),
        ] {
            assert_eq!(serde_json::to_string(&aid)?, wire);
            assert_eq!(serde_json::from_str::<Aid>(wire)?, aid);
            assert_eq!(rules::Aid::try_from(aid), Ok(program));
        }
        // Every other key name is read, as the one key the schema leaves out.
        for wire in ["\"Enter\"", "\"pf5\"", "\"PF4\"", "\"PF3\"", "\"\""] {
            assert_eq!(serde_json::from_str::<Aid>(wire)?, Aid::Unserved, "{wire}");
        }
        assert!(serde_json::from_str::<Aid>("5").is_err());
        assert!(serde_json::to_string(&Aid::Unserved).is_err());
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

    #[test]
    fn a_member_is_read_once_an_unknown_one_is_skipped_and_a_required_one_is_never_absent() {
        // Every member of a `TranAddCopiedFields` but the dates, each as `"name":value`.
        let copied_members = [
            r#""type_cd":"01""#,
            r#""cat_cd":"0001""#,
            r#""source":"POS TERM  ""#,
            r#""description":"D""#,
            r#""amount":"+00000012.34""#,
            r#""merchant_id":"800000000""#,
            r#""merchant_name":"N""#,
            r#""merchant_city":"C""#,
            r#""merchant_zip":"72112     ""#,
        ];
        let all = copied_members.join(",");
        // A request, or copied fields, holding `extra` after the members each requires.
        let request = |extra: &str| format!(r#"{{"aid":"ENTER","fields":{{}}{extra}}}"#);
        let copied = |extra: &str| format!("{{{all}{extra}}}");
        let without = |name: &str| {
            let key = format!("\"{name}\":");
            let kept: Vec<&str> = copied_members
                .iter()
                .copied()
                .filter(|entry| !entry.starts_with(&key))
                .collect();
            format!("{{{}}}", kept.join(","))
        };
        for (text, accepted) in [
            (request(r#","page":{"key":[1,null]},"x":0"#), true),
            (request(r#","aid":"PF5""#), false),
            (request(r#","acct_id":null,"acct_id":null"#), false),
            (
                request(&format!(r#","card_num":"{CARD}","card_num":null"#)),
                false,
            ),
            (request(r#","fields":{}"#), false),
            (request(r#","confirm":"Y","confirm":"N""#), false),
            (request(r#","confirm":null"#), false),
        ] {
            let read = serde_json::from_str::<TranAddRequest>(&text);
            assert_eq!(read.is_ok(), accepted, "{text}");
        }
        for (text, accepted) in [
            (r#"{"type_cd":"01","tran_id":["0000000000000301"]}"#, true),
            (r#"{"amount":"1","amount":"2"}"#, false),
            (r#"{"merchant_zip":null}"#, false),
        ] {
            let read = serde_json::from_str::<TranAddFields>(text);
            assert_eq!(read.is_ok(), accepted, "{text}");
        }
        for (text, accepted) in [
            (copied(""), true),
            (copied(r#","orig_date":null,"proc_date":null,"x":{}"#), true),
            (
                copied(r#","orig_date":"2022-06-10","orig_date":null"#),
                false,
            ),
            (copied(r#","proc_date":"2022-13-40""#), false),
            (copied(r#","source":"OPERATOR  ""#), false),
            (without("type_cd"), false),
            (without("merchant_zip"), false),
        ] {
            let read = serde_json::from_str::<TranAddCopiedFields>(&text);
            assert_eq!(read.is_ok(), accepted, "{text}");
        }
    }

    #[test]
    fn each_type_serialises_its_members_by_name_in_declaration_order() -> TestResult {
        let request: TranAddRequest = serde_json::from_value(full_body())?;
        let (source, description) = ("S".repeat(10), "D".repeat(60));
        let (name, city, zip) = ("N".repeat(30), "C".repeat(25), "Z".repeat(10));
        assert_eq!(
            serde_json::to_string(&request)?,
            format!(
                r#"{{"aid":"ENTER","acct_id":"00000000050","fields":{{"type_cd":"01","cat_cd":"0001","source":"{source}","description":"{description}","amount":"+00000012.34","orig_date":"2022-06-10","proc_date":"2022-06-11","merchant_id":"800000000","merchant_name":"{name}","merchant_city":"{city}","merchant_zip":"{zip}"}},"confirm":"Y"}}"#
            )
        );

        let copied = TranAddCopiedFields {
            proc_date: None,
            ..TranAddCopiedFields::from(&validated()?)
        };
        let (source, description) = (
            padded("POS TERM", 10),
            padded("Purchase at Abshire-Lowe", 60),
        );
        let (name, city, zip) = (
            padded("Abshire-Lowe", 30),
            padded("North Enoshaven", 25),
            padded("72112", 10),
        );
        let copied_text = format!(
            r#"{{"type_cd":"01","cat_cd":"0001","source":"{source}","description":"{description}","amount":"+00000012.34","orig_date":"2022-06-10","proc_date":null,"merchant_id":"800000000","merchant_name":"{name}","merchant_city":"{city}","merchant_zip":"{zip}"}}"#
        );
        assert_eq!(serde_json::to_string(&copied)?, copied_text);

        let prompt = TranAddResult {
            copied: Some(copied),
            ..TranAddResult::from(rules::AddOutcome::prompt(filled_key()?, validated()?, None))
        };
        assert_eq!(
            serde_json::to_string(&prompt)?,
            format!(
                r#"{{"acct_id":"00000000050","card_num":"{CARD_REDACTED}","copied":{copied_text},"field":"confirm","message":"Confirm to add this transaction...","new_tran_id":null}}"#
            )
        );
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
    #[test]
    fn each_input_is_held_to_its_map_length() -> TestResult {
        block_on(async {
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
        })
    }

    // audit: COTRN02C-007
    #[test]
    fn an_unserved_key_gets_the_invalid_key_answer_with_no_field() -> TestResult {
        block_on(async {
            for aid in ["PF4", "PF7", "enter", ""] {
                let mut body = full_body();
                *body.pointer_mut("/aid").ok_or("aid")? = json!(aid);
                let request = extract(&body)
                    .await?
                    .map_err(|error| format!("{aid:?} refused by the extractor: {error:?}"))?;
                assert_eq!(request.aid, Aid::Unserved, "{aid}");
                let refused = rules::Aid::try_from(request.aid).err().ok_or(aid)?;
                assert_eq!(refused, rules::AddFailure::invalid_key(), "{aid}");
                let error = ScreenError::from(refused);
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (StatusCode::BAD_REQUEST, CCDA_MSG_INVALID_KEY, None),
                    "{aid}"
                );
                assert_eq!(error.message().len(), 50);
                assert_eq!(
                    error.message(),
                    format!("{:<50}", "Invalid key pressed. Please see below...")
                );
            }
            Ok(())
        })
    }

    #[test]
    fn an_aid_that_names_no_key_leaves_the_body_unreadable() -> TestResult {
        block_on(async {
            let generic = (StatusCode::BAD_REQUEST, INVALID_BODY.to_owned(), None);
            let mut missing = full_body();
            missing.as_object_mut().ok_or("body")?.remove("aid");
            let mut bodies = vec![missing];
            for aid in [json!(5), Value::Null, json!(["PF4"]), json!({"key": "PF4"})] {
                let mut body = full_body();
                *body.pointer_mut("/aid").ok_or("aid")? = aid;
                bodies.push(body);
            }
            for body in bodies {
                let error = extract(&body).await?.err().ok_or("accepted")?;
                assert_eq!(error, generic, "{body}");
            }
            let truncated = r#"{"aid": "PF4", "fields": {}"#.to_owned();
            let error = extract_text(truncated).await?.err().ok_or("accepted")?;
            assert_eq!(error, generic);
            Ok(())
        })
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
