//! The contract of transaction `CPVD`, program COPAUS1C and map `COPAU1A` (AAP 0.3.4.1):
//!
//! - endpoint 22, `GET /api/v1/authorizations/{acct_id}/{auth_key}`, ENTER and F8: query
//!   [`AuthDetailQuery`], response [`AuthDetailResponse`];
//! - endpoint 23, `POST /api/v1/authorizations/{acct_id}/{auth_key}/fraud`, F5: no body,
//!   response [`FraudToggleResult`]. COPAUS1C links COPAUS2C for the `AUTHFRDS` write, so this
//!   module is COPAUS2C's contract too.
//!
//! F3 back to `/authorizations` is the screen's own (COPAUS1C-004). The two path segments stay
//! raw strings of the handler: `domain::online::copaus1c::edit` edits the account and
//! `domain::online::copaus0c::AuthKey::parse` the key, both total, inside the service.
//!
//! # Values
//!
//! Every display field is the string COPAUS1C moves into its map field, already padded to the
//! field's width by `domain::online::copaus1c`, so neither this module nor the screen formats
//! anything (COPAUS1C-012 to COPAUS1C-023). The card number is a [`CardNumber`], whose
//! `Serialize` emits only twelve `*` and the last four digits (COPAUS1C-012, E32). Every field
//! that shows stored request text, which CP00 keeps as the request sent it and which can hold a
//! card number or an SSN, passes through [`redact_free_text`]: the source, the MCC code, the
//! auth type, the transaction ID, the match status and the merchant's name, ID, city, state and
//! ZIP. A written number of nine digits in any of them shows as `***-**-` and its last four,
//! one of sixteen as twelve `*` and its last four, and one of any other count of ten or more
//! with `*` for each digit but its last four, so the transaction ID `TXN000000000001` shows as
//! `TXN********0001` (COPAUS1C-020, COPAUS1C-021, COPAUS1C-023, E32). The other fields are
//! values the program builds, shown as built, and the examples hold no protected value
//! (Cardholder Data Redaction).
//!
//! A failure answers through `ScreenError`, never through these types: the 400, 404 and 500 of
//! `domain::online::copaus1c::AuthDetailFailure` (COPAUS1C-006, COPAUS1C-009 to COPAUS1C-011,
//! COPAUS1C-026, COPAUS1C-031, COPAUS1C-033).

use core::fmt;

use domain::CardNumber;
use domain::online::copaus1c as rules;
use domain::online::copaus1c::FraudToggleResult as ToggleOutcome;
use domain::sensitive::redact_free_text;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{PageDirection, digits};

/// The query of endpoint 22: which key the screen sent (COPAUS1C-003).
///
/// - `direction` absent: ENTER, a first entry from COPAUS0C's selection, or a refresh. The
///   authorization the path names is read and shown (COPAUS1C-008).
/// - `direction=forward`: F8, `PROCESS-PF8-KEY`. The authorization after the one the path names
///   is shown, or the same one with `Already at the last Authorization...` (COPAUS1C-024,
///   COPAUS1C-025).
///
/// `backward` is no key of this screen. It deserialises, so the request is not rejected here,
/// and the service answers it as the `WHEN OTHER` branch of `EVALUATE EIBAID` does, with ENTER's
/// processing. The query takes no `page_size`: the endpoint addresses one authorization, not a
/// collection (Bounded Pagination).
// The schema name `AuthDetailQuery` is fixed (Generated API Types Only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ToSchema, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct AuthDetailQuery {
    /// `forward` for F8, the next authorization; absent for ENTER.
    pub direction: Option<PageDirection>,
}

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

/// Its one member, `direction`, `null` when absent.
impl Serialize for AuthDetailQuery {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthDetailQuery", 1)?;
        out.serialize_field("direction", &self.direction)?;
        out.end()
    }
}

/// The members of [`AuthDetailQuery`], in declaration order.
const AUTH_DETAIL_QUERY_MEMBERS: &[&str] = &["direction"];

/// `direction` read once, absent or `null` as ENTER; any other member, `page_size` included, is
/// skipped unread.
impl<'de> Deserialize<'de> for AuthDetailQuery {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct(
            "AuthDetailQuery",
            AUTH_DETAIL_QUERY_MEMBERS,
            AuthDetailQueryVisitor,
        )
    }
}

/// The visitor of [`AuthDetailQuery`]'s members.
struct AuthDetailQueryVisitor;

impl<'de> Visitor<'de> for AuthDetailQueryVisitor {
    type Value = AuthDetailQuery;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("struct AuthDetailQuery")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<AuthDetailQuery, A::Error> {
        let mut direction = None;
        while let Some(name) = map.next_key_seed(WireName(AUTH_DETAIL_QUERY_MEMBERS))? {
            match name {
                Some("direction") => member(&mut map, &mut direction, "direction")?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AuthDetailQuery {
            direction: direction.flatten(),
        })
    }
}

/// One authorization as COPAUS1C's `POPULATE-AUTH-DETAILS` fills `COPAU1A`, the key the screen
/// keeps for it, and the message row (COPAUS1C-012 to COPAUS1C-025, COPAUS1C-034, COPAUS1C-035).
///
/// Each text member holds exactly its map field's width in characters, trailing spaces
/// included, except a masked member whose redacted SSN needs more room than its trailing spaces
/// give: it is longer by the difference, as an SSN that fills the 9 bytes of the ZIP shows as
/// its 11-character redacted form, one more than `MERZIP` ([`redact_free_text`]). It has no
/// `Deserialize`: the card number it carries is redacted.
// The schema name `AuthDetailResponse` is fixed (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
pub struct AuthDetailResponse {
    /// The account, `CDEMO-ACCT-ID PIC 9(11)`: its 11 digits with the leading zeros. The screen
    /// keeps it for the next request (COPAUS1C-003).
    #[schema(example = "00000012345")]
    pub acct_id: String,
    /// The shown authorization's key, `PA-AUTHORIZATION-KEY`, as the 16-digit `{auth_key}` path
    /// segment of endpoints 22 and 23. After F8 it is the next authorization's key, which the
    /// screen keeps; after ENTER, F5 and F8 at the last authorization it is unchanged
    /// (COPAUS1C-003, COPAUS1C-024, COPAUS1C-025, COPAUS1C-034).
    pub auth_key: String,
    /// Card #, `CARDNUM` (16): twelve `*` and the card's last four digits, never the full
    /// number (COPAUS1C-012, COPAU01-013, E32).
    #[schema(value_type = String, example = "************5740")]
    pub card_num: CardNumber,
    /// Auth Date, `AUTHDT` (10): `MM/DD/YY` from the stored `YYMMDD`, then two spaces
    /// (COPAUS1C-013).
    // Display text, not a typed date: the stored `PA-AUTH-ORIG-DATE` is `PIC X(06)` with no
    // century, outside the `X(10)` dates Typed Dates and Timestamps covers, and the program
    // checks no calendar, so a stored value can name no date (COPAU01-015).
    #[schema(example = "01/31/24  ")]
    pub auth_date: String,
    /// Auth Time, `AUTHTM` (10): `HH:MM:SS` from the stored `HHMMSS`, then two spaces
    /// (COPAUS1C-014).
    // Display text for the reason given at `auth_date`: `PA-AUTH-ORIG-TIME` is `PIC X(06)`.
    #[schema(example = "10:30:45  ")]
    pub auth_time: String,
    /// Auth Resp, `AUTHRSP` (1): `A` for response code `00`, `D` otherwise (COPAUS1C-016).
    #[schema(example = "A")]
    pub response: String,
    /// Resp Reason, `AUTHRSN` (20): the reason code, `-` and its description, or
    /// `9999-ERROR` for a code outside the table (COPAUS1C-017, COPAUS1C-018).
    #[schema(example = "0000-APPROVED       ")]
    pub reason: String,
    /// Auth Code, `AUTHCD` (6): the processing code's six digits (COPAUS1C-019).
    pub auth_code: String,
    /// Amount, `AUTHAMT` (12): the approved amount edited by `-zzzzzzz9.99`, as the domain
    /// renders it (COPAUS1C-015).
    #[schema(example = "     1234.56")]
    pub amount: String,
    /// POS Entry Mode, `POSEMD` (4): the two digits, then two spaces (COPAUS1C-020).
    pub pos_entry_mode: String,
    /// Source, `AUTHSRC` (10): `PA-MESSAGE-SOURCE`, then four spaces, masked by
    /// [`redact_free_text`] as stored request text (COPAUS1C-020, E32).
    pub source: String,
    /// MCC Code, `MCCCD` (4): `PA-MERCHANT-CATAGORY-CODE`, masked as the source is
    /// (COPAUS1C-020, E32).
    pub merchant_category_code: String,
    /// Card Exp. Date, `CRDEXP` (5): `MM/YY` text built from the four stored bytes of
    /// `PA-CARD-EXPIRY-DATE PIC X(04)`, not a date (COPAUS1C-020).
    pub card_expiry: String,
    /// Auth Type, `AUTHTYP` (14): `PA-AUTH-TYPE`, then ten spaces, masked as the transaction ID
    /// is (COPAUS1C-021, E32).
    pub auth_type: String,
    /// Tran Id, `TRNID` (15): `PA-TRANSACTION-ID`, masked by [`redact_free_text`] as stored
    /// request text: an SSN in it shows as `***-**-` and its last four digits, and the 12-digit
    /// run of `TXN000000000001` as `TXN********0001` (COPAUS1C-021, E32).
    pub transaction_id: String,
    /// Match Status, `AUTHMTC` (1): the stored byte, masked as the transaction ID is
    /// (COPAUS1C-021, E32).
    pub match_status: String,
    /// Fraud Status, `AUTHFRD` (10): `F` or `R`, `-` and the report date, or `-` and nine
    /// spaces when the authorization was never reported (COPAUS1C-022).
    pub fraud_status: String,
    /// Name, `MERNAME` (25): `PA-MERCHANT-NAME`, then three spaces, masked by
    /// [`redact_free_text`] as stored request text, so a card number or an SSN in the name shows
    /// only in its redacted form (COPAUS1C-023, E32).
    pub merchant_name: String,
    /// Merchant ID, `MERID` (15): `PA-MERCHANT-ID`, masked as the name is (COPAUS1C-023, E32).
    pub merchant_id: String,
    /// City, `MERCITY` (25): `PA-MERCHANT-CITY`, then twelve spaces, masked as the name is
    /// (COPAUS1C-023, E32).
    pub merchant_city: String,
    /// State, `MERST` (2): `PA-MERCHANT-STATE`, masked as the name is (COPAUS1C-023, E32).
    pub merchant_state: String,
    /// Zip, `MERZIP` (10): `PA-MERCHANT-ZIP`, then one space, masked as the name is. An SSN
    /// that fills the 9 bytes shows as its 11-character redacted form, one character more than
    /// the map field (COPAUS1C-023, E32).
    pub merchant_zip: String,
    /// The message row, `ERRMSG` (78): none after ENTER or a found F8 (COPAUS1C-035),
    /// `Already at the last Authorization...` after F8 at the account's last authorization
    /// (COPAUS1C-025), `AUTH MARKED FRAUD...` or `AUTH FRAUD REMOVED...` after a committed F5
    /// (COPAUS1C-032).
    #[schema(example = "Already at the last Authorization...")]
    pub message: Option<String>,
}

/// Every member in declaration order: the card number in its redacted form, the message `null`
/// when there is none.
impl Serialize for AuthDetailResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AuthDetailResponse", 23)?;
        out.serialize_field("acct_id", &self.acct_id)?;
        out.serialize_field("auth_key", &self.auth_key)?;
        out.serialize_field("card_num", &self.card_num)?;
        out.serialize_field("auth_date", &self.auth_date)?;
        out.serialize_field("auth_time", &self.auth_time)?;
        out.serialize_field("response", &self.response)?;
        out.serialize_field("reason", &self.reason)?;
        out.serialize_field("auth_code", &self.auth_code)?;
        out.serialize_field("amount", &self.amount)?;
        out.serialize_field("pos_entry_mode", &self.pos_entry_mode)?;
        out.serialize_field("source", &self.source)?;
        out.serialize_field("merchant_category_code", &self.merchant_category_code)?;
        out.serialize_field("card_expiry", &self.card_expiry)?;
        out.serialize_field("auth_type", &self.auth_type)?;
        out.serialize_field("transaction_id", &self.transaction_id)?;
        out.serialize_field("match_status", &self.match_status)?;
        out.serialize_field("fraud_status", &self.fraud_status)?;
        out.serialize_field("merchant_name", &self.merchant_name)?;
        out.serialize_field("merchant_id", &self.merchant_id)?;
        out.serialize_field("merchant_city", &self.merchant_city)?;
        out.serialize_field("merchant_state", &self.merchant_state)?;
        out.serialize_field("merchant_zip", &self.merchant_zip)?;
        out.serialize_field("message", &self.message)?;
        out.end()
    }
}

/// The view `domain::online::copaus1c` builds for ENTER, F8 and a committed F5, field by field;
/// the account and the key take the text the screen sends back in the path.
impl From<rules::AuthDetailView> for AuthDetailResponse {
    fn from(view: rules::AuthDetailView) -> Self {
        let rules::AuthDetailView {
            acct_id,
            key,
            card_num,
            auth_date,
            auth_time,
            response,
            reason,
            auth_code,
            amount,
            pos_entry_mode,
            source,
            merchant_category_code,
            card_expiry,
            auth_type,
            transaction_id,
            match_status,
            fraud_status,
            merchant_name,
            merchant_id,
            merchant_city,
            merchant_state,
            merchant_zip,
            message,
        } = view;
        Self {
            acct_id: digits(acct_id),
            auth_key: key.render(),
            card_num,
            auth_date,
            auth_time,
            response: response.as_str().to_owned(),
            reason,
            auth_code,
            amount,
            pos_entry_mode,
            // Every member that shows stored request text leaves through `redact_free_text`,
            // whatever its width: CP00 keeps that text as the request sent it, so the 22 bytes of
            // the name can hold a card number, the 15 of either ID and the 13 of the city an
            // SSN, and the 9 of the ZIP an SSN that fills them. No stored text reaches the body,
            // or the derived `Debug` of this type and of `FraudToggleResult`, unmasked; the
            // shorter members hold no written number of nine digits and pass unchanged. The
            // values the program builds (the dates and times, the response, reason, auth code,
            // amount, POS entry mode, expiry and fraud status) stay as built.
            source: redact_free_text(&source),
            merchant_category_code: redact_free_text(&merchant_category_code),
            card_expiry,
            auth_type: redact_free_text(&auth_type),
            transaction_id: redact_free_text(&transaction_id),
            match_status: redact_free_text(&match_status),
            fraud_status,
            merchant_name: redact_free_text(&merchant_name),
            merchant_id: redact_free_text(&merchant_id),
            merchant_city: redact_free_text(&merchant_city),
            merchant_state: redact_free_text(&merchant_state),
            merchant_zip: redact_free_text(&merchant_zip),
            message: message.map(str::to_owned),
        }
    }
}

/// The answer of F5, endpoint 23: `MARK-AUTH-FRAUD`, COPAUS2C's `AUTHFRDS` write and the
/// replace of the detail (COPAUS1C-028 to COPAUS1C-034).
///
/// After a commit, `message` is `AUTH MARKED FRAUD...` or `AUTH FRAUD REMOVED...` and `detail`
/// is the same authorization as replaced, its new fraud status and report date shown and its key
/// unchanged (COPAUS1C-032, COPAUS1C-034).
// The schema name `FraudToggleResult` is fixed (Generated API Types Only). The domain outcome of
// the same name is imported as `ToggleOutcome`, so the document holds one schema of that name.
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
#[schema(example = json!({"message": "AUTH MARKED FRAUD...", "detail": null}))]
pub struct FraudToggleResult {
    /// The message row: the domain outcome's message (COPAUS1C-032, COPAUS1C-035).
    pub message: String,
    /// The authorization as replaced, after a commit; absent when the outcome carries no
    /// detail.
    pub detail: Option<AuthDetailResponse>,
}

/// Both members in declaration order, `detail` `null` when the outcome carries none.
impl Serialize for FraudToggleResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("FraudToggleResult", 2)?;
        out.serialize_field("message", &self.message)?;
        out.serialize_field("detail", &self.detail)?;
        out.end()
    }
}

/// A committed F5's view, the `Ok` of `domain::online::copaus1c::FraudToggleResult::into_result`:
/// its message row and the authorization as replaced.
impl From<rules::AuthDetailView> for FraudToggleResult {
    fn from(view: rules::AuthDetailView) -> Self {
        let detail = AuthDetailResponse::from(view);
        Self {
            message: detail.message.clone().unwrap_or_default(),
            detail: Some(detail),
        }
    }
}

/// The domain's end of F5, `domain::online::copaus1c::toggle_outcome`, as the response body.
///
/// A committed toggle gives its message and its view. A rolled-back toggle is a 500: the service
/// returns its `AuthDetailFailure` as the error, which `ScreenError` answers (COPAUS1C-031,
/// COPAUS1C-033), so a 200 never carries one. Should one reach this conversion, the body holds
/// the failure's message and no detail, and never a stored value.
impl From<ToggleOutcome> for FraudToggleResult {
    fn from(outcome: ToggleOutcome) -> Self {
        match outcome.into_result() {
            Ok(view) => Self::from(view),
            Err(failure) => Self {
                message: failure.message().to_owned(),
                detail: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::FromRequestParts;
    use axum::http::{Request, StatusCode};
    use domain::copybook::{PicS, PicU, PicX};
    use domain::money::{Decimal, Money};
    use domain::online::copaus0c::AuthKey;
    use domain::online::copaus1c::{self as rules, DetailLookup, NextLookup, SegmentUpdate};
    use domain::online::copaus2c::FraudOutcome;
    use domain::records::PendingAuthDetailRecord;
    use domain::{CardNumber, Ssn};
    use serde_json::json;
    use test_support::fixtures;
    use utoipa::{IntoParams, PartialSchema};

    use super::{AuthDetailQuery, AuthDetailResponse, FraudToggleResult};
    use crate::dto::{INVALID_QUERY, PageDirection, ValidQuery};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number outside the fixture corpus, its first twelve digits and its redacted form.
    const CARD: &str = "1234567890123456";
    const CARD_HEAD: &str = "123456789012";
    const CARD_REDACTED: &str = "************3456";

    /// An invented account and its 11-digit text.
    const ACCT: u64 = 12_345;
    const ACCT_TEXT: &str = "00000012345";

    /// The inverted keys CP00 stores for 2024-01-31 at 10:30:45.123, and the next older time.
    const DATE_9C: i64 = 75_968;
    const TIME_9C: i64 = 896_954_876;
    const NEXT_TIME_9C: i64 = 896_954_877;

    /// The `PA-FRAUD-RPT-DATE` COPAUS2C stamps, `MM/DD/YY`.
    const REPORT_DATE: &str = "01/31/24";

    fn acct() -> Result<PicU<11>, Box<dyn StdError>> {
        Ok(PicU::new(ACCT)?)
    }

    /// An approved detail as CP00 writes it, its fraud flag blank.
    fn record() -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(DATE_9C)?,
            auth_time_9c: PicS::new(TIME_9C)?,
            orig_date: PicX::from_text("240131")?,
            orig_time: PicX::from_text("103045")?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("SALE")?,
            card_expiry_date: PicX::from_text("0927")?,
            message_type: PicX::from_text("AUTH")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("103045")?,
            auth_resp_code: PicX::from_text("00")?,
            auth_resp_reason: PicX::from_text("0000")?,
            processing_code: PicU::new(1_234)?,
            transaction_amt: Money::new(Decimal::new(99_999, 2))?,
            approved_amt: Money::new(Decimal::new(123_456, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCHANT0000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_text("P")?,
            auth_fraud: PicX::spaces(),
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// The response ENTER gives for `stored`.
    fn shown(stored: &PendingAuthDetailRecord) -> Result<AuthDetailResponse, Box<dyn StdError>> {
        Ok(rules::detail(acct()?, DetailLookup::Found(stored))?.into())
    }

    /// The query string `uri` as endpoint 22's extractor reads it.
    async fn query(uri: &str) -> Result<Result<AuthDetailQuery, StatusCode>, axum::http::Error> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(
            ValidQuery::<AuthDetailQuery>::from_request_parts(&mut head, &())
                .await
                .map(|ValidQuery(value)| value)
                .map_err(|error| {
                    if error.message() == INVALID_QUERY {
                        error.status()
                    } else {
                        StatusCode::INTERNAL_SERVER_ERROR
                    }
                }),
        )
    }

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    // `domain::online::copaus1c` proves the display values of COPAUS1C-013 to COPAUS1C-023 for
    // its view; this test proves that the response carries them unchanged, at the map widths.
    #[test]
    fn enter_carries_every_field_at_its_map_width() -> TestResult {
        let stored = record()?;
        let response = shown(&stored)?;
        assert_eq!(response.acct_id, ACCT_TEXT);
        assert_eq!(response.auth_key, AuthKey::of(&stored).render());
        assert_eq!(response.auth_date, "01/31/24  ");
        assert_eq!(response.auth_time, "10:30:45  ");
        assert_eq!(response.response, "A");
        assert_eq!(response.reason, "0000-APPROVED       ");
        assert_eq!(response.amount, "     1234.56");
        assert_eq!(response.card_expiry, "09/27");
        assert_eq!(response.fraud_status, "-         ");
        assert_eq!(response.message, None);
        // The `LENGTH` of each map field (`modern/audit/screens.md`, COPAU01-013 to COPAU01-052),
        // and the 11 digits of the account and 16 of the key the path carries.
        let widths = [
            ("acct_id", &response.acct_id, 11),
            ("auth_key", &response.auth_key, AuthKey::RENDERED_WIDTH),
            ("auth_date", &response.auth_date, 10),
            ("auth_time", &response.auth_time, 10),
            ("response", &response.response, 1),
            ("reason", &response.reason, 20),
            ("auth_code", &response.auth_code, 6),
            ("amount", &response.amount, 12),
            ("pos_entry_mode", &response.pos_entry_mode, 4),
            ("source", &response.source, 10),
            (
                "merchant_category_code",
                &response.merchant_category_code,
                4,
            ),
            ("card_expiry", &response.card_expiry, 5),
            ("auth_type", &response.auth_type, 14),
            ("transaction_id", &response.transaction_id, 15),
            ("match_status", &response.match_status, 1),
            ("fraud_status", &response.fraud_status, 10),
            ("merchant_name", &response.merchant_name, 25),
            ("merchant_id", &response.merchant_id, 15),
            ("merchant_city", &response.merchant_city, 25),
            ("merchant_state", &response.merchant_state, 2),
            ("merchant_zip", &response.merchant_zip, 10),
        ];
        for (name, value, width) in widths {
            assert_eq!(value.chars().count(), width, "{name}: {value:?}");
        }
        Ok(())
    }

    // audit: COPAUS1C-012
    #[test]
    fn the_card_number_leaves_only_redacted() -> TestResult {
        let response = shown(&record()?)?;
        let body = serde_json::to_value(&response)?;
        assert_eq!(body.get("card_num"), Some(&json!(CARD_REDACTED)));
        let debug = format!("{response:?}");
        let toggled = FraudToggleResult::from(rules::toggle_outcome(
            acct()?,
            &rules::toggle(&record()?).with_report_date(PicX::from_text(REPORT_DATE)?),
            FraudOutcome::Added,
            Some(SegmentUpdate::Replaced),
        ));
        let toggled_body = serde_json::to_string(&toggled)?;
        for text in [
            body.to_string(),
            debug,
            toggled_body,
            format!("{toggled:?}"),
        ] {
            assert!(text.contains(CARD_REDACTED), "{text}");
            assert!(!text.contains(CARD_HEAD), "{text}");
        }
        Ok(())
    }

    /// The response a committed F5 gives for `stored`: marked as fraud on [`REPORT_DATE`], the
    /// detail replaced.
    fn toggle_response(
        stored: &PendingAuthDetailRecord,
    ) -> Result<FraudToggleResult, Box<dyn StdError>> {
        Ok(FraudToggleResult::from(rules::toggle_outcome(
            acct()?,
            &rules::toggle(stored).with_report_date(PicX::from_text(REPORT_DATE)?),
            FraudOutcome::Added,
            Some(SegmentUpdate::Replaced),
        )))
    }

    /// The members of `response` that show stored request text, each with its name and the
    /// `LENGTH` of its map field (`modern/audit/screens.md`), in the order of
    /// `ordinary_request_text`.
    fn request_text(response: &AuthDetailResponse) -> [(&'static str, &str, usize); 10] {
        [
            ("source", response.source.as_str(), 10),
            (
                "merchant_category_code",
                response.merchant_category_code.as_str(),
                4,
            ),
            ("auth_type", response.auth_type.as_str(), 14),
            ("transaction_id", response.transaction_id.as_str(), 15),
            ("match_status", response.match_status.as_str(), 1),
            ("merchant_name", response.merchant_name.as_str(), 25),
            ("merchant_id", response.merchant_id.as_str(), 15),
            ("merchant_city", response.merchant_city.as_str(), 25),
            ("merchant_state", response.merchant_state.as_str(), 2),
            ("merchant_zip", response.merchant_zip.as_str(), 10),
        ]
    }

    /// What `request_text` shows for `record()`: each stored value padded to its map field,
    /// the 12-digit run of the transaction ID masked and every other value unchanged.
    fn ordinary_request_text() -> [String; 10] {
        [
            format!("{:<10}", "POS"),
            "5411".to_owned(),
            format!("{:<14}", "SALE"),
            "TXN********0001".to_owned(),
            "P".to_owned(),
            format!("{:<25}", "CORNER GROCERY"),
            "MERCHANT0000001".to_owned(),
            format!("{:<25}", "SEATTLE"),
            "WA".to_owned(),
            format!("{:<10}", "98101"),
        ]
    }

    /// Asserts that each member of `response` that shows stored request text, and the member of
    /// the same name in its JSON `body`, holds the text `shown_otherwise` names for it, or else
    /// its `ordinary_request_text`, and is as many characters longer than its map field as
    /// `shown_otherwise` says (0 for the ordinary text). No assertion message prints a value.
    fn check_request_text(
        (case, owner): (&str, &str),
        response: &AuthDetailResponse,
        body: &serde_json::Value,
        shown_otherwise: &[(&str, String, usize)],
    ) {
        for ((field, value, width), ordinary) in request_text(response)
            .into_iter()
            .zip(ordinary_request_text())
        {
            let (expected, longer) = shown_otherwise
                .iter()
                .find(|(name, ..)| *name == field)
                .map_or((ordinary.as_str(), 0), |(_, text, longer)| {
                    (text.as_str(), *longer)
                });
            assert!(
                value == expected,
                "{case}: {owner}.{field} is not the expected text"
            );
            assert!(
                body.get(field).and_then(serde_json::Value::as_str) == Some(expected),
                "{case}: {owner} JSON member {field} is not the expected text"
            );
            assert_eq!(
                value.chars().count(),
                width + longer,
                "{case}: {owner}.{field} width"
            );
        }
    }

    /// Asserts that neither the JSON nor the `Debug` text of `detail` and of `toggled` holds any
    /// of `needles`. No assertion message prints one.
    fn check_egress(
        case: &str,
        detail: &AuthDetailResponse,
        toggled: &FraudToggleResult,
        needles: &[String],
    ) -> TestResult {
        let egress = [
            ("AuthDetailResponse JSON", serde_json::to_string(detail)?),
            ("AuthDetailResponse Debug", format!("{detail:?}")),
            ("FraudToggleResult JSON", serde_json::to_string(toggled)?),
            ("FraudToggleResult Debug", format!("{toggled:?}")),
        ];
        for (form, text) in &egress {
            let leaks = needles
                .iter()
                .filter(|needle| text.contains(needle.as_str()))
                .count();
            assert_eq!(leaks, 0, "{case}: {form} holds a protected value");
        }
        Ok(())
    }

    // audit: COPAUS1C-012
    // audit: COPAUS1C-020
    // audit: COPAUS1C-021
    // audit: COPAUS1C-023
    #[test]
    fn stored_request_text_leaves_with_its_card_numbers_and_ssns_redacted() -> TestResult {
        // Protected values (Cardholder Data Redaction): search needles only, and no assertion
        // message below prints one or the text that would hold it.
        let cards = fixtures::fixture_card_numbers()?;
        let ssn_forms = fixtures::fixture_ssn_forms()?;
        // The 50 card numbers of `carddata.txt`; the 50 SSNs of `custdata.txt`, each plain and
        // dashed.
        assert_eq!((cards.len(), ssn_forms.len()), (50, 100));
        let needles: Vec<String> = cards.iter().chain(&ssn_forms).cloned().collect();
        let card = cards.first().ok_or("carddata.txt holds a card number")?;
        let ssn = ssn_forms
            .iter()
            .find(|form| form.len() == 9)
            .ok_or("custdata.txt holds a 9-digit SSN")?;
        let dashed_ssn = ssn_forms
            .iter()
            .find(|form| form.len() == 11)
            .ok_or("custdata.txt holds a dashed SSN")?;
        // `************` and the last four digits; `***-**-` and the last four digits.
        let card_redacted = CardNumber::parse(card)?.redacted();
        let ssn_redacted = Ssn::parse(ssn)?.redacted();
        let dashed_redacted = Ssn::parse(&dashed_ssn.replace('-', ""))?.redacted();
        // Each case: the stored detail, and each member it shows otherwise than the ordinary
        // record does, with how many characters that member is longer than its map field. The
        // IDs keep 15 characters through the 6 trailing spaces a 9-digit SSN leaves, the name
        // and the city 25; the SSN filling the 9 bytes of the ZIP leaves 1 trailing space for
        // the 2 characters its redacted form adds.
        let cases = [
            ("the ordinary record", record()?, vec![]),
            (
                "a plain SSN in the merchant ID",
                PendingAuthDetailRecord {
                    merchant_id: PicX::from_text(ssn)?,
                    ..record()?
                },
                vec![("merchant_id", format!("{ssn_redacted:<15}"), 0)],
            ),
            (
                "a plain SSN in the transaction ID",
                PendingAuthDetailRecord {
                    transaction_id: PicX::from_text(ssn)?,
                    ..record()?
                },
                vec![("transaction_id", format!("{ssn_redacted:<15}"), 0)],
            ),
            (
                "a plain SSN filling the ZIP",
                PendingAuthDetailRecord {
                    merchant_zip: PicX::from_text(ssn)?,
                    ..record()?
                },
                vec![("merchant_zip", ssn_redacted.clone(), 1)],
            ),
            (
                "a dashed SSN in the city",
                PendingAuthDetailRecord {
                    merchant_city: PicX::from_text(dashed_ssn)?,
                    ..record()?
                },
                vec![("merchant_city", format!("{dashed_redacted:<25}"), 0)],
            ),
            (
                "a card number in the name",
                PendingAuthDetailRecord {
                    merchant_name: PicX::from_text(card)?,
                    ..record()?
                },
                vec![("merchant_name", format!("{card_redacted:<25}"), 0)],
            ),
            (
                "a plain SSN in the name",
                PendingAuthDetailRecord {
                    merchant_name: PicX::from_text(ssn)?,
                    ..record()?
                },
                vec![("merchant_name", format!("{ssn_redacted:<25}"), 0)],
            ),
        ];
        for (case, stored, shown_otherwise) in cases {
            let detail = shown(&stored)?;
            let toggled = toggle_response(&stored)?;
            let replaced = toggled
                .detail
                .as_ref()
                .ok_or("a committed toggle carries its detail")?;
            let toggled_body = serde_json::to_value(&toggled)?;
            check_request_text(
                (case, "AuthDetailResponse"),
                &detail,
                &serde_json::to_value(&detail)?,
                &shown_otherwise,
            );
            check_request_text(
                (case, "FraudToggleResult.detail"),
                replaced,
                toggled_body
                    .get("detail")
                    .ok_or("a committed toggle serialises its detail")?,
                &shown_otherwise,
            );
            check_egress(case, &detail, &toggled, &needles)?;
        }
        Ok(())
    }

    // audit: COPAUS1C-024
    // audit: COPAUS1C-025
    #[test]
    fn f8_shows_the_next_key_or_keeps_the_current_one_at_the_last() -> TestResult {
        let stored = record()?;
        let following = PendingAuthDetailRecord {
            auth_time_9c: PicS::new(NEXT_TIME_9C)?,
            ..record()?
        };
        let found = AuthDetailResponse::from(rules::next(
            acct()?,
            &stored,
            NextLookup::Found(&following),
        )?);
        assert_eq!(found.auth_key, AuthKey::of(&following).render());
        assert_eq!(found.message, None);
        let last = AuthDetailResponse::from(rules::next(acct()?, &stored, NextLookup::End)?);
        assert_eq!(last.auth_key, AuthKey::of(&stored).render());
        assert_eq!(last.message.as_deref(), Some(rules::ALREADY_AT_LAST));
        let body = serde_json::to_value(&last)?;
        assert_eq!(body.get("message"), Some(&json!(rules::ALREADY_AT_LAST)));
        Ok(())
    }

    // audit: COPAUS1C-028
    // audit: COPAUS1C-032
    // audit: COPAUS1C-034
    #[test]
    fn a_committed_toggle_carries_its_message_and_the_replaced_detail() -> TestResult {
        let cases = [
            (PicX::<1>::spaces(), rules::AUTH_MARKED_FRAUD, "F-01/31/24"),
            (
                PicX::from_text("F")?,
                rules::AUTH_FRAUD_REMOVED,
                "R-01/31/24",
            ),
        ];
        for (flag, message, fraud_status) in cases {
            let stored = PendingAuthDetailRecord {
                auth_fraud: flag,
                ..record()?
            };
            let toggle = rules::toggle(&stored).with_report_date(PicX::from_text(REPORT_DATE)?);
            let outcome = rules::toggle_outcome(
                acct()?,
                &toggle,
                FraudOutcome::Added,
                Some(SegmentUpdate::Replaced),
            );
            assert!(outcome.commits());
            let from_view = FraudToggleResult::from(outcome.clone().into_result()?);
            let result = FraudToggleResult::from(outcome);
            assert_eq!(result, from_view);
            assert_eq!(result.message, message);
            let detail = result
                .detail
                .ok_or("a committed toggle carries its detail")?;
            assert_eq!(detail.fraud_status, fraud_status);
            assert_eq!(detail.message.as_deref(), Some(message));
            assert_eq!(detail.auth_key, AuthKey::of(&stored).render());
            assert_eq!(detail.card_num.redacted(), CARD_REDACTED);
        }
        Ok(())
    }

    // audit: COPAUS1C-031
    // audit: COPAUS1C-033
    #[test]
    fn a_rolled_back_toggle_carries_the_failure_message_and_no_detail() -> TestResult {
        let toggle = rules::toggle(&record()?).with_report_date(PicX::from_text(REPORT_DATE)?);
        let cases = [
            (FraudOutcome::Added, Some(SegmentUpdate::Failed)),
            (FraudOutcome::Added, None),
            (FraudOutcome::InsertFailed, None),
            (FraudOutcome::UpdateFailed, Some(SegmentUpdate::Replaced)),
        ];
        for (fraud, segment_update) in cases {
            let outcome = rules::toggle_outcome(acct()?, &toggle, fraud, segment_update);
            assert!(!outcome.commits(), "{fraud:?} {segment_update:?}");
            let message = outcome.message().to_owned();
            let result = FraudToggleResult::from(outcome);
            assert!(!result.message.is_empty(), "{fraud:?} {segment_update:?}");
            assert_eq!(result.message, message);
            assert_eq!(result.detail, None);
            assert_eq!(
                serde_json::to_value(&result)?.get("detail"),
                Some(&json!(null))
            );
        }
        Ok(())
    }

    // audit: COPAUS1C-003
    #[test]
    fn the_query_is_absent_for_enter_and_forward_for_f8() -> TestResult {
        block_on(async {
            let cases = [
                ("/detail", None),
                ("/detail?direction=forward", Some(PageDirection::Forward)),
                ("/detail?direction=backward", Some(PageDirection::Backward)),
                // Not a collection: a `page_size` is no member, so it is ignored, never a 422.
                ("/detail?page_size=101", None),
            ];
            for (uri, direction) in cases {
                let parsed = query(uri)
                    .await?
                    .map_err(|status| format!("{uri}: {status}"))?;
                assert_eq!(parsed, AuthDetailQuery { direction }, "{uri}");
            }
            assert_eq!(
                query("/detail?direction=sideways").await?,
                Err(StatusCode::BAD_REQUEST)
            );
            Ok(())
        })
    }

    /// A query string that repeats `direction` does not deserialise, whatever the two values.
    #[test]
    fn a_repeated_direction_is_refused() -> TestResult {
        block_on(async {
            for uri in [
                "/detail?direction=forward&direction=forward",
                "/detail?direction=backward&direction=forward",
            ] {
                assert_eq!(query(uri).await?, Err(StatusCode::BAD_REQUEST), "{uri}");
            }
            Ok(())
        })
    }

    /// The query's JSON form: `direction` read once, absent and `null` alike as ENTER, and any
    /// other member skipped wherever and however often it appears.
    #[test]
    fn the_query_reads_direction_once_and_skips_other_members() -> TestResult {
        let duplicate = "duplicate field `direction`";
        let cases = [
            ("{}", Ok(None)),
            (r#"{"direction":null}"#, Ok(None)),
            (
                r#"{"direction":"forward"}"#,
                Ok(Some(PageDirection::Forward)),
            ),
            (
                r#"{"page_size":101,"direction":"backward","page_size":7}"#,
                Ok(Some(PageDirection::Backward)),
            ),
            (
                r#"{"direction":"forward","direction":"forward"}"#,
                Err(duplicate),
            ),
            (
                r#"{"direction":null,"direction":"backward"}"#,
                Err(duplicate),
            ),
        ];
        for (wire, expected) in cases {
            match (serde_json::from_str::<AuthDetailQuery>(wire), expected) {
                (Ok(read), Ok(direction)) => {
                    assert_eq!(read, AuthDetailQuery { direction }, "{wire}");
                }
                (Err(error), Err(refusal)) => {
                    assert!(error.to_string().starts_with(refusal), "{wire}: {error}");
                }
                (read, expected) => {
                    return Err(format!("{wire}: read {read:?}, expected {expected:?}").into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn the_schemas_show_a_redacted_card_and_one_query_parameter() -> TestResult {
        let response = serde_json::to_value(AuthDetailResponse::schema())?;
        assert_eq!(
            response.pointer("/properties/card_num/type"),
            Some(&json!("string"))
        );
        assert_eq!(
            response.pointer("/properties/card_num/example"),
            Some(&json!("************5740"))
        );
        for display_text in ["auth_date", "auth_time", "card_expiry"] {
            let pointer = format!("/properties/{display_text}/type");
            assert_eq!(
                response.pointer(&pointer),
                Some(&json!("string")),
                "{pointer}"
            );
        }
        let params = serde_json::to_value(AuthDetailQuery::into_params(|| None))?;
        let [direction] = params.as_array().map(Vec::as_slice).unwrap_or_default() else {
            return Err(format!("one query parameter expected: {params}").into());
        };
        assert_eq!(direction.get("name"), Some(&json!("direction")));
        assert_eq!(direction.get("in"), Some(&json!("query")));
        assert_eq!(direction.get("required"), Some(&json!(false)));
        let schema = direction
            .get("schema")
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            schema.contains("#/components/schemas/PageDirection"),
            "{schema}"
        );
        Ok(())
    }

    /// The JSON members each type writes: its schema's properties, in declaration order, an
    /// absent value written as `null`.
    #[test]
    fn every_type_writes_its_schema_members_in_declaration_order() -> TestResult {
        let cases: [(&str, String, serde_json::Value, &[&str]); 3] = [
            (
                "AuthDetailQuery",
                serde_json::to_string(&AuthDetailQuery::default())?,
                serde_json::to_value(AuthDetailQuery::schema())?,
                &["direction"],
            ),
            (
                "AuthDetailResponse",
                serde_json::to_string(&shown(&record()?)?)?,
                serde_json::to_value(AuthDetailResponse::schema())?,
                &[
                    "acct_id",
                    "auth_key",
                    "card_num",
                    "auth_date",
                    "auth_time",
                    "response",
                    "reason",
                    "auth_code",
                    "amount",
                    "pos_entry_mode",
                    "source",
                    "merchant_category_code",
                    "card_expiry",
                    "auth_type",
                    "transaction_id",
                    "match_status",
                    "fraud_status",
                    "merchant_name",
                    "merchant_id",
                    "merchant_city",
                    "merchant_state",
                    "merchant_zip",
                    "message",
                ],
            ),
            (
                "FraudToggleResult",
                serde_json::to_string(&toggle_response(&record()?)?)?,
                serde_json::to_value(FraudToggleResult::schema())?,
                &["message", "detail"],
            ),
        ];
        for (name, text, schema, members) in cases {
            let names = |object: Option<&serde_json::Map<String, serde_json::Value>>| {
                let mut keys = object
                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                keys.sort_unstable();
                keys
            };
            let mut listed = members
                .iter()
                .copied()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            listed.sort_unstable();
            let written = serde_json::from_str::<serde_json::Value>(&text)?;
            assert_eq!(
                names(written.as_object()),
                listed,
                "{name}: written members"
            );
            let declared = schema
                .get("properties")
                .and_then(serde_json::Value::as_object);
            assert_eq!(names(declared), listed, "{name}: schema properties");
            // The first occurrence of each member's key: an outer member precedes the members
            // of the detail `FraudToggleResult` nests after its message.
            let positions = members
                .iter()
                .map(|member| text.find(&format!("\"{member}\":")))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{name}: a member is not written"))?;
            assert!(
                positions.is_sorted(),
                "{name}: members out of declaration order"
            );
        }
        Ok(())
    }
}
