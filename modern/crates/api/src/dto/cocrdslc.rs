//! COCRDSLC, the card detail: the contracts of endpoint 8, `GET /api/v1/cards/view`
//! (transaction `CCDL`, map `CCRDSLA`, route `/cards/view`; AAP 0.3.4.1).
//!
//! [`CardLookupQuery`] is the query string of ENTER: the account input `ACCTSID` and the card,
//! either as the typed `CARDSID` or as the `cards.card_ref` handle of a COCRDLI list row
//! (COCRDSLC-006). Endpoint 9 (`dto::cocrdupc`) takes the same query. [`CardViewResponse`] is the
//! card `1200-SETUP-SCREEN-VARS` shows for a found card (COCRDSLC-018 to COCRDSLC-023); every
//! other outcome is a `ScreenError`, mapped by `crate::error`. F3 is the screen's own and sends no
//! request.
//!
//! # The single card-detail endpoint
//!
//! Endpoint 8 is the one operation that may return all 16 digits of a card number (Cardholder
//! Data Redaction, AAP 0.3.4.8), and this is the only `api` module besides `services::cocrdslc`
//! that may name `DisclosedCardNumber`: `grep -rn DisclosedCardNumber modern/crates/api/src`
//! matches these two files and no other.
//!
//! The choice between the full and the redacted number is not made here, nor by any request
//! field. `api::services::cocrdslc` passes the verified token's `usr_type` to the domain, whose
//! `disclose` returns the full number for `A` (COCRDSLC-022) and twelve `*` followed by the last
//! four digits for every other type (COCRDSLC-023, PIC-024); the owner clause of the rule never
//! grants disclosure, because `CardDemo` links no user to a card. This module carries the
//! domain's variant into [`CardViewNumber`] unchanged and never calls `expose()` or `disclose()`.
//!
//! # Redaction of the request
//!
//! A typed `card_num` is request input, not egress. It is still kept out of every output this
//! module produces: the `Debug` of [`CardLookupQuery`] masks it, its `Serialize` omits it, and its
//! documented example is the redacted form. No DTO here has a CVV field (COCRDSLC-021,
//! CVACT02Y-003).
//!
//! # Validation
//!
//! The DTO enforces only the map lengths, the server half of the `both` rows COCRDSL-013
//! (`ACCTSID`, 11) and COCRDSL-016 (`CARDSID`, 16). Everything else is COCRDSLC's own edit, run
//! by the service through `domain::online::cocrdslc::edit` so that its messages and their
//! precedence stay the program's: a blank or non-numeric input, and the rule that exactly one of
//! `card_num` and `card_ref` names the card (COCRDSLC-007 to COCRDSLC-013). The endpoint returns
//! a single resource and takes no `page_size`.

use core::fmt;

use chrono::NaiveDate;
use domain::online::cocrdslc as rules;
use domain::sensitive::{CardNumber, DisclosedCardNumber};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{digits, full_text};

/// What the `Debug` of [`CardLookupQuery`] shows for a typed `card_num`, whatever it holds: the
/// input need not be a card number at all, so no character of it is kept.
const CARD_NUM_MASK: &str = "****************";

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

/// The query string of endpoint 8 (and of endpoint 9): the account and the card to show
/// (COCRDSLC-006 to COCRDSLC-013).
///
/// Every member is the raw text of its screen field. An absent `acct_id` is the empty text, which
/// COCRDSLC's edit answers with `'Account number not provided'` or `'No input received'`. The
/// card is named by `card_num`, the typed number, or by `card_ref`, the handle a COCRDLI list
/// row or an earlier response carries (COCRDSLC-026); the domain edit, not this DTO, decides
/// that exactly one of the two is supplied, so its messages stay the program's.
///
/// `Debug` masks `card_num` and `Serialize` omits it (Cardholder Data Redaction).
// The schema name `CardLookupQuery` is the frontend's `components["schemas"]["CardLookupQuery"]`
// (Generated API Types Only); `api::openapi::ApiDoc` registers it.
#[derive(Clone, ToSchema, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct CardLookupQuery {
    /// `ACCTSID` (`ACCTSIDI PIC X(11)`): the account number, 11 digits (COCRDSL-013).
    #[validate(length(max = 11))]
    #[schema(required = false, max_length = 11, example = "00000000050")]
    #[param(required = false, max_length = 11, example = "00000000050")]
    pub acct_id: String,
    /// `CARDSID` (`CARDSIDI PIC X(16)`): the card number as typed, 16 digits (COCRDSL-016).
    /// Never sent together with `card_ref`.
    #[validate(length(max = 16))]
    #[schema(write_only, max_length = 16, example = "************5740")]
    #[param(write_only, max_length = 16, example = "************5740")]
    pub card_num: Option<String>,
    /// The `cards.card_ref` handle of the card (CVACT02Y-008), from a COCRDLI list row or from
    /// an earlier [`CardViewResponse`] (COCRDSLC-006, COCRDSLC-026). Never sent together with
    /// `card_num`.
    #[schema(example = 1)]
    #[param(example = 1)]
    pub card_ref: Option<i64>,
}

/// `acct_id` and `card_ref`, with `card_num` masked whatever it holds.
impl fmt::Debug for CardLookupQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CardLookupQuery")
            .field("acct_id", &self.acct_id)
            .field("card_num", &self.card_num.as_ref().map(|_| CARD_NUM_MASK))
            .field("card_ref", &self.card_ref)
            .finish()
    }
}

/// `acct_id` and `card_ref` only: a typed `card_num` is never serialised.
// Hand-written rather than derived with `#[serde(skip_serializing)]` on `card_num`: utoipa 5
// reads that attribute as `skip` and would drop `card_num` from the documented query parameters
// and from the `CardLookupQuery` schema, taking the typed number out of the endpoint's contract.
impl Serialize for CardLookupQuery {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut query = serializer.serialize_struct("CardLookupQuery", 2)?;
        query.serialize_field("acct_id", &self.acct_id)?;
        query.serialize_field("card_ref", &self.card_ref)?;
        query.end()
    }
}

/// The members of [`CardLookupQuery`], in declaration order.
const CARD_LOOKUP_QUERY_MEMBERS: &[&str] = &["acct_id", "card_num", "card_ref"];

/// Each member once, in any order: an absent `acct_id` is the empty text, an absent or `null`
/// card input is `None`, and a member the query does not declare is skipped.
impl<'de> Deserialize<'de> for CardLookupQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "CardLookupQuery",
            CARD_LOOKUP_QUERY_MEMBERS,
            CardLookupQueryVisitor,
        )
    }
}

/// The visitor of [`CardLookupQuery`]'s members.
struct CardLookupQueryVisitor;

impl<'de> Visitor<'de> for CardLookupQueryVisitor {
    type Value = CardLookupQuery;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct CardLookupQuery")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<CardLookupQuery, A::Error> {
        let mut acct_id = None;
        let mut card_num = None;
        let mut card_ref = None;
        while let Some(name) = map.next_key_seed(WireName(CARD_LOOKUP_QUERY_MEMBERS))? {
            match name {
                Some("acct_id") => member(&mut map, &mut acct_id, "acct_id")?,
                Some("card_num") => member(&mut map, &mut card_num, "card_num")?,
                Some("card_ref") => member(&mut map, &mut card_ref, "card_ref")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(CardLookupQuery {
            acct_id: acct_id.unwrap_or_default(),
            card_num: card_num.flatten(),
            card_ref: card_ref.flatten(),
        })
    }
}

/// The card number of a [`CardViewResponse`], as the domain's disclosure decided it: either
/// variant serialises as a plain 16-character string.
///
/// - [`CardViewNumber::Full`]: all 16 characters of `CARD-NUM`, for a verified `usr_type` of
///   `A` (COCRDSLC-022).
/// - [`CardViewNumber::Redacted`]: twelve `*` and the last four digits, for every other user
///   type (COCRDSLC-023, PIC-024).
///
/// `Debug` of both variants is the redacted form. The type is no schema of the API document:
/// the response field that holds it is documented as a `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardViewNumber {
    /// The full number, cleared for disclosure by the domain.
    Full(DisclosedCardNumber),
    /// The redacted number.
    Redacted(CardNumber),
}

/// The number the variant holds, through that number's own `Serialize`, with no variant name.
impl Serialize for CardViewNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Full(number) => number.serialize(serializer),
            Self::Redacted(number) => number.serialize(serializer),
        }
    }
}

/// The domain's variant, one to one: the disclosure is decided before this point.
impl From<rules::CardNumberDisclosure> for CardViewNumber {
    fn from(disclosure: rules::CardNumberDisclosure) -> Self {
        match disclosure {
            rules::CardNumberDisclosure::Full(number) => Self::Full(number),
            rules::CardNumberDisclosure::Redacted(number) => Self::Redacted(number),
        }
    }
}

/// The 200 body of endpoint 8: the found card as `1200-SETUP-SCREEN-VARS` shows it
/// (COCRDSLC-018 to COCRDSLC-023).
///
/// It holds no card verification code: the program moves the verification-code field of
/// `CVACT02Y` to no screen field (COCRDSLC-021).
// The schema name `CardViewResponse` is fixed (Generated API Types Only); `api::openapi::ApiDoc`
// registers it.
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
pub struct CardViewResponse {
    /// `ACCTSIDO`: the account the request named, 11 digits with their leading zeros, never the
    /// card's own account (COCRDSLC-014, COCRDSLC-018).
    #[schema(example = "00000000050")]
    pub acct_id: String,
    /// The card's `cards.card_ref` handle, with which the screen reads the same card again while
    /// the card input shows the returned number (COCRDSLC-018, COCRDSLC-026).
    #[schema(example = 1)]
    pub card_ref: i64,
    /// `CARDSIDO`: the card number, all 16 digits for an administrator token and otherwise
    /// twelve `*` followed by the last four digits (COCRDSLC-022, COCRDSLC-023).
    #[schema(value_type = String, example = "************5740")]
    pub card_num: CardViewNumber,
    /// `CRDNAMEO` from `CARD-EMBOSSED-NAME PIC X(50)`: its 50 stored bytes, trailing spaces
    /// included (COCRDSLC-019).
    pub embossed_name: String,
    /// `CARD-EXPIRAION-DATE PIC X(10)` as a date, `null` when the stored date is blank
    /// (COCRDSLC-019, CVACT02Y-005). The screen shows its month in `EXPMON` and its year in
    /// `EXPYEAR` (COCRDSLC-024).
    pub expiration_date: Option<NaiveDate>,
    /// `CRDSTCDO` from `CARD-ACTIVE-STATUS PIC X(01)`: its stored byte, a space when it is a
    /// space (COCRDSLC-019).
    #[schema(example = "Y")]
    pub status: String,
    /// `ERRMSGO`: always `null`, because a found card leaves `WS-RETURN-MSG` spaces; every other
    /// outcome is a `ScreenError` (COCRDSLC-018).
    pub message: Option<String>,
    /// `INFOMSGO` (40): the information message of a found card, `'   Displaying requested
    /// details'` with its three leading spaces (COCRDSLC-020).
    pub info_message: Option<String>,
}

/// Every member in declaration order, `null` for an absent one.
impl Serialize for CardViewResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("CardViewResponse", 8)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("card_ref", &self.card_ref)?;
        out.serialize_field("card_num", &self.card_num)?;
        out.serialize_field("embossed_name", &self.embossed_name)?;
        out.serialize_field("expiration_date", &self.expiration_date)?;
        out.serialize_field("status", &self.status)?;
        out.serialize_field("message", &self.message)?;
        out.serialize_field("info_message", &self.info_message)?;
        out.end()
    }
}

/// The fields of the domain's view, rendered as the map shows them.
impl From<rules::CardView> for CardViewResponse {
    fn from(view: rules::CardView) -> Self {
        Self {
            acct_id: digits(view.acct_id),
            card_ref: view.card_ref.get(),
            card_num: view.card_num.into(),
            embossed_name: full_text(&view.embossed_name),
            expiration_date: view.expiration_date,
            status: full_text(&view.status),
            message: None,
            info_message: Some(view.info_message.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::FromRequestParts;
    use axum::http::{Request, StatusCode};
    use chrono::NaiveDate;
    use domain::copybook::{PicU, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::online::cocrdslc as rules;
    use domain::online::cosgn00c::UserType;
    use domain::sensitive::CardNumber;
    use serde_json::{Value, json};
    use utoipa::{IntoParams, PartialSchema};
    use validator::Validate;

    use super::{CardLookupQuery, CardViewNumber, CardViewResponse};
    use crate::dto::{FIELD_LENGTH_MESSAGE, INVALID_QUERY, ValidQuery};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// A synthetic card number, in no fixture.
    const CARD: &str = "1111222233334444";

    /// [`CARD`] as twelve `*` and its last four digits.
    const REDACTED: &str = "************4444";

    /// The view of a found card of account 50, handle 7, as `user_type` is shown it.
    fn view(
        user_type: &UserType,
        expiry: Option<NaiveDate>,
    ) -> Result<rules::CardView, Box<dyn StdError>> {
        Ok(rules::CardView {
            acct_id: PicU::new(50)?,
            card_ref: CardRef::new(7),
            card_num: rules::disclose(user_type, &CardNumber::parse(CARD)?),
            embossed_name: PicX::from_text("Test Holder")?,
            expiration_date: expiry,
            status: PicX::from_text("Y")?,
            info_message: rules::FOUND_CARDS_FOR_ACCOUNT,
        })
    }

    fn expiry() -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2025, 4, 30).ok_or_else(|| "2025-04-30 is a date".into())
    }

    /// `query` extracted through [`ValidQuery`], as the handler receives it.
    async fn extract(
        query: &str,
    ) -> Result<Result<CardLookupQuery, ScreenError>, axum::http::Error> {
        let uri = format!("/api/v1/cards/view?{query}");
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<CardLookupQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(value)| value),
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

    // audit: COCRDSLC-018
    // audit: COCRDSLC-019
    // audit: COCRDSLC-020
    // audit: COCRDSLC-022
    // audit: COCRDSLC-023
    #[test]
    fn the_response_shows_the_number_the_domain_disclosed_and_nothing_else() -> TestResult {
        let embossed_name = format!("{:<50}", "Test Holder");
        for (user_type, card_num) in [(UserType::USER, REDACTED), (UserType::ADMIN, CARD)] {
            let response = CardViewResponse::from(view(&user_type, Some(expiry()?))?);
            assert_eq!(
                serde_json::to_value(&response)?,
                json!({
                    "acct_id": "00000000050",
                    "card_ref": 7,
                    "card_num": card_num,
                    "embossed_name": embossed_name,
                    "expiration_date": "2025-04-30",
                    "status": "Y",
                    "message": null,
                    "info_message": "   Displaying requested details",
                }),
                "{card_num}"
            );
            // Debug hides the number even when the body discloses it.
            let debug = format!("{response:?}");
            assert!(!debug.contains(CARD), "{debug}");
            assert!(debug.contains(REDACTED), "{debug}");
        }
        Ok(())
    }

    // audit: COCRDSLC-019
    #[test]
    fn the_name_and_the_status_are_their_stored_bytes() -> TestResult {
        let full_name = "X".repeat(50);
        for (name, status) in [("Test Holder", " "), ("", "N"), (full_name.as_str(), "Y")] {
            let response = CardViewResponse::from(rules::CardView {
                embossed_name: PicX::from_text(name)?,
                status: PicX::from_text(status)?,
                ..view(&UserType::USER, None)?
            });
            assert_eq!(response.embossed_name.chars().count(), 50, "{name:?}");
            let body = serde_json::to_value(&response)?;
            assert_eq!(
                body["embossed_name"],
                json!(format!("{name:<50}")),
                "{name:?}"
            );
            assert_eq!(body["status"], json!(status), "{status:?}");
        }
        Ok(())
    }

    // audit: COCRDSLC-019
    #[test]
    fn a_blank_expiry_date_is_null() -> TestResult {
        let response = CardViewResponse::from(view(&UserType::USER, None)?);
        assert_eq!(response.expiration_date, None);
        assert_eq!(
            serde_json::to_value(&response)?["expiration_date"],
            Value::Null
        );
        Ok(())
    }

    #[test]
    fn each_disclosure_maps_to_its_own_variant() -> TestResult {
        let number = CardNumber::parse(CARD)?;
        assert_eq!(
            CardViewNumber::from(rules::disclose(&UserType::ADMIN, &number)),
            CardViewNumber::Full(number.disclose())
        );
        assert_eq!(
            CardViewNumber::from(rules::disclose(&UserType::USER, &number)),
            CardViewNumber::Redacted(number.clone())
        );
        assert_eq!(
            serde_json::to_value(CardViewNumber::Redacted(number))?,
            json!(REDACTED)
        );
        Ok(())
    }

    // audit: COCRDSLC-021
    #[test]
    fn no_body_or_schema_names_a_cvv_and_the_number_is_a_string() -> TestResult {
        let mut found = Vec::new();
        keys(
            &serde_json::to_value(CardViewResponse::from(view(&UserType::ADMIN, None)?))?,
            &mut found,
        );
        let response_schema = serde_json::to_value(CardViewResponse::schema())?;
        keys(&response_schema, &mut found);
        keys(
            &serde_json::to_value(CardLookupQuery::schema())?,
            &mut found,
        );
        assert!(found.iter().all(|key| !key.contains("cvv")), "{found:?}");
        let card_num = &response_schema["properties"]["card_num"];
        assert_eq!(card_num["type"], json!("string"));
        assert_eq!(card_num["example"], json!("************5740"));
        assert_eq!(
            response_schema["properties"]["expiration_date"]["format"],
            json!("date")
        );
        Ok(())
    }

    #[test]
    fn the_typed_number_is_documented_write_only_with_a_redacted_example() -> TestResult {
        let params = serde_json::to_value(CardLookupQuery::into_params(|| None))?;
        let names: Vec<&str> = params
            .as_array()
            .ok_or("parameters are a list")?
            .iter()
            .filter_map(|param| param["name"].as_str())
            .collect();
        assert_eq!(names, ["acct_id", "card_num", "card_ref"]);
        for param in params.as_array().ok_or("parameters are a list")? {
            assert_eq!(param["in"], json!("query"), "{param}");
            assert_ne!(param["required"], json!(true), "{param}");
        }
        assert_eq!(params[1]["schema"]["writeOnly"], json!(true));
        assert_eq!(params[1]["example"], json!("************5740"));
        assert_eq!(params[1]["schema"]["maxLength"], json!(16));
        assert_eq!(params[0]["schema"]["maxLength"], json!(11));

        let schema = serde_json::to_value(CardLookupQuery::schema())?;
        assert_eq!(schema["properties"]["card_num"]["writeOnly"], json!(true));
        assert_eq!(
            schema["properties"]["card_num"]["example"],
            json!("************5740")
        );
        Ok(())
    }

    #[test]
    fn the_typed_number_never_leaves_through_debug_or_serialize() -> TestResult {
        let query = CardLookupQuery {
            acct_id: "00000000050".to_owned(),
            card_num: Some(CARD.to_owned()),
            card_ref: None,
        };
        let debug = format!("{query:?}");
        assert!(!debug.contains("4444"), "{debug}");
        assert!(debug.contains("****************"), "{debug}");
        assert_eq!(
            serde_json::to_value(&query)?,
            json!({"acct_id": "00000000050", "card_ref": null})
        );
        assert!(
            format!(
                "{:?}",
                CardLookupQuery {
                    card_num: None,
                    ..query
                }
            )
            .contains("card_num: None")
        );
        Ok(())
    }

    // audit: COCRDSL-013
    // audit: COCRDSL-016
    #[test]
    fn the_query_is_deserialised_then_held_to_the_map_lengths() -> TestResult {
        block_on(async {
            let by_ref = extract("acct_id=00000000050&card_ref=7").await??;
            assert_eq!(
                (by_ref.acct_id.as_str(), by_ref.card_num, by_ref.card_ref),
                ("00000000050", None, Some(7))
            );
            let by_number = extract(&format!("acct_id=00000000050&card_num={CARD}")).await??;
            assert_eq!(by_number.card_num.as_deref(), Some(CARD));
            assert_eq!(by_number.card_ref, None);

            // Absent inputs reach the domain edit, which owns their messages.
            let empty = extract("").await??;
            assert_eq!(
                (empty.acct_id.as_str(), empty.card_num, empty.card_ref),
                ("", None, None)
            );
            let both =
                extract(&format!("acct_id=00000000050&card_num={CARD}&card_ref=7")).await??;
            assert!(both.validate().is_ok());

            let too_long = format!("{CARD}5");
            for (query, field) in [
                ("acct_id=000000000500&card_ref=7".to_owned(), "acct_id"),
                (
                    format!("acct_id=00000000050&card_num={too_long}"),
                    "card_num",
                ),
            ] {
                let error = extract(&query).await?.err().ok_or("a length is exceeded")?;
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(field)),
                    "{query}"
                );
            }

            let error = extract("acct_id=00000000050&card_ref=ABC")
                .await?
                .err()
                .ok_or("a handle is an integer")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, INVALID_QUERY, None)
            );
            Ok(())
        })
    }

    /// A query string naming a member twice is a 400, and a name the query does not declare is
    /// read past.
    #[test]
    fn a_repeated_member_is_refused_and_an_undeclared_one_is_skipped() -> TestResult {
        block_on(async {
            for query in [
                "acct_id=00000000050&acct_id=00000000051&card_ref=7",
                "acct_id=00000000050&card_num=4444&card_num=4444",
                "acct_id=00000000050&card_ref=7&card_ref=8",
            ] {
                let error = extract(query).await?.err().ok_or("a member is repeated")?;
                assert_eq!(
                    (error.status(), error.message(), error.field()),
                    (StatusCode::BAD_REQUEST, INVALID_QUERY, None),
                    "{query}"
                );
            }
            let query =
                extract("page_key=3&acct_id=00000000050&direction=x&card_ref=7&flag").await??;
            assert_eq!(
                (query.acct_id.as_str(), query.card_num, query.card_ref),
                ("00000000050", None, Some(7))
            );
            Ok(())
        })
    }

    /// `null` for either card input reads as no input.
    #[test]
    fn a_null_card_input_reads_as_absent() -> TestResult {
        let query: CardLookupQuery = serde_json::from_value(json!({
            "acct_id": "00000000050",
            "card_num": null,
            "card_ref": null,
        }))?;
        assert_eq!(
            (query.acct_id.as_str(), query.card_num, query.card_ref),
            ("00000000050", None, None)
        );
        Ok(())
    }

    /// The body's text lists the eight members in their declaration order.
    #[test]
    fn the_body_names_every_member_in_declaration_order() -> TestResult {
        let embossed_name = format!("{:<50}", "Test Holder");
        let response = CardViewResponse::from(view(&UserType::USER, Some(expiry()?))?);
        assert_eq!(
            serde_json::to_string(&response)?,
            format!(
                r#"{{"acct_id":"00000000050","card_ref":7,"card_num":"{REDACTED}","embossed_name":"{embossed_name}","expiration_date":"2025-04-30","status":"Y","message":null,"info_message":"   Displaying requested details"}}"#
            )
        );
        Ok(())
    }
}
