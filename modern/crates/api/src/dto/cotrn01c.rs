//! COTRN01C, the transaction view: the response contract of endpoint 12,
//! `GET /api/v1/transactions/{tran_id}` (transaction `CT01`, map `COTRN1A`; AAP 0.3.4.1).
//!
//! The request has neither body nor query. The Tran ID input `TRNIDIN` is the `tran_id` path
//! parameter, a [`TranIdPath`], which the handler extracts with the parent module's `ValidPath`.
//! It revalidates the map `LENGTH` of `TRNIDIN`, 16, and nothing else (COTRN01-013): a longer
//! segment is a 400 `Input does not fit the screen field...` naming `tran_id`, and nothing is
//! read. A text that fits reaches the service unparsed and untrimmed, so that
//! `domain::online::cotrn01c::edit_tran_id` answers a blank one with the program's own `Tran ID
//! can NOT be empty...` (COTRN01C-009) and no deserialiser decides it. F3, F4 and F5 send no
//! request (COTRN01C-004 to COTRN01C-006).
//!
//! A successful read answers [`TranViewResponse`], one value per detail field of the map
//! (COTRN01C-015 to COTRN01C-028). Every failure past the length check answers the
//! `ScreenError` that `crate::error` builds from `rules::TranViewFailure`: 400, 404 `Transaction
//! ID NOT found...` or 500, each on field `tran_id` and with no transaction field (COTRN01C-009,
//! COTRN01C-012, COTRN01C-013).
//!
//! # Values
//!
//! The response carries what the screen shows, so the screen formats nothing but the two
//! timestamps:
//!
//! - Text fields hold every byte of their map field, trailing spaces included, as
//!   `rules::TranView` cut them to the map width: `TRNSRC` is `POS TERM  `, ten characters.
//! - `TCATCD` and `MID` hold their digits with the leading zeros, `0005` for category 5.
//! - The amount is the 12-character `WS-TRAN-AMT PIC +99999999.99` string.
//! - The card number is a [`CardNumber`], which serialises only as twelve `*` and its last four
//!   characters, for every user type (COTRN01C-017, E32, Cardholder Data Redaction).
//! - `TRAN-ORIG-TS` and `TRAN-PROC-TS` are typed timestamps (schema format `date-time`), `null`
//!   when the stored field is blank. Their layout code is a storage detail and is not sent
//!   (COTRN01C-023, COTRN01C-024, E33, Typed Dates and Timestamps).

use chrono::NaiveDateTime;
use domain::CardNumber;
use domain::copybook::PicX;
use domain::online::cotrn01c as rules;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

/// The path of endpoint 12, `/api/v1/transactions/{tran_id}`: the Tran ID input `TRNIDIN` as
/// typed.
///
/// Only the map `LENGTH` is checked here, the server half of the `both` row COTRN01-013 of
/// `modern/audit/screens.md`: a segment longer than 16 characters is a 400 with
/// [`super::FIELD_LENGTH_MESSAGE`] naming `tran_id`, and nothing is read. Anything that fits,
/// blank and sixteen `\0` included, reaches the domain's edit unparsed and untrimmed, so the
/// program's own messages answer it: `Tran ID can NOT be empty...` for a field of spaces or of
/// low-values (COTRN01C-009), otherwise the key exactly as entered (COTRN01C-010), and
/// `Transaction ID NOT found...` when no row has it (COTRN01C-012). No `PIC X` rule
/// ([`super::pic_x`]) applies: sixteen `\0` must reach the blank edit, and a key holding a byte
/// no stored key holds is the domain's not-found outcome.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Path)]
pub struct TranIdPath {
    /// Transaction ID as typed on the screen: at most 16 characters, neither trimmed nor
    /// zero-filled (COTRN01C-010).
    #[validate(length(max = 16))]
    #[param(max_length = 16)]
    pub tran_id: String,
}

/// The transaction as the `COTRN1A` map shows it after a successful read: the 200 body of
/// endpoint 12 (COTRN01C-015 to COTRN01C-028).
///
/// The body is the same for a `U` and an `A` token, because COTRN01C checks no user type
/// (COTRN01C-015). It has no `Deserialize`: it holds a [`CardNumber`], which no request may
/// carry back.
// The schema name `TranViewResponse` is the frontend's `components["schemas"]["TranViewResponse"]`
// (Generated API Types Only); renaming the type changes the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct TranViewResponse {
    /// `TRNID`, from `TRAN-ID PIC X(16)`: the record's 16-byte key as stored (COTRN01C-016).
    pub tran_id: String,
    /// `CARDNUM`, from `TRAN-CARD-NUM PIC X(16)`: twelve `*` and the last four characters of the
    /// stored field, never the full value; the COBOL moves all 16 bytes (COTRN01C-017, E32).
    #[schema(value_type = String, example = "************5740")]
    pub card_num: CardNumber,
    /// `TTYPCD`, from `TRAN-TYPE-CD PIC X(02)`: the 2 bytes as stored (COTRN01C-018).
    pub type_cd: String,
    /// `TCATCD PIC X(4)`, from `TRAN-CAT-CD PIC 9(04)`: the four digits with their leading
    /// zeros, category 5 as `0005` (COTRN01C-019).
    pub cat_cd: String,
    /// `TRNSRC`, from `TRAN-SOURCE PIC X(10)`: the 10 bytes as stored (COTRN01C-020).
    pub source: String,
    /// `TDESC PIC X(60)`, from `TRAN-DESC PIC X(100)`: its first 60 bytes (COTRN01C-021).
    pub description: String,
    /// `TRNAMT PIC X(12)`, from `TRAN-AMT PIC S9(09)V99` through `WS-TRAN-AMT PIC +99999999.99`:
    /// `+00000050.47`, `-00000012.00`, and `+23456789.01` for 123456789.01, whose ninth integer
    /// digit the picture has no position for (COTRN01C-022, X-COTRN01C-1).
    pub amount: String,
    /// `TORIGDT`, from `TRAN-ORIG-TS PIC X(26)`: the typed timestamp in whichever layout it was
    /// stored, `null` when the field is blank. The screen shows its date part, the ten bytes the
    /// COBOL moves (COTRN01C-023).
    pub orig_ts: Option<NaiveDateTime>,
    /// `TPROCDT`, from `TRAN-PROC-TS PIC X(26)`: as `orig_ts`; a transaction posted by
    /// `posttran` carries its posting instant (COTRN01C-024).
    pub proc_ts: Option<NaiveDateTime>,
    /// `MID PIC X(9)`, from `TRAN-MERCHANT-ID PIC 9(09)`: the nine digits with their leading
    /// zeros (COTRN01C-025).
    pub merchant_id: String,
    /// `MNAME PIC X(30)`, from `TRAN-MERCHANT-NAME PIC X(50)`: its first 30 bytes
    /// (COTRN01C-026).
    pub merchant_name: String,
    /// `MCITY PIC X(25)`, from `TRAN-MERCHANT-CITY PIC X(50)`: its first 25 bytes
    /// (COTRN01C-027).
    pub merchant_city: String,
    /// `MZIP`, from `TRAN-MERCHANT-ZIP PIC X(10)`: the 10 bytes as stored (COTRN01C-028).
    pub merchant_zip: String,
    /// `ERRMSG`, the message row: always `null`, because every pass clears `WS-MESSAGE` and a
    /// successful read moves no message into it (COTRN01C-015).
    pub message: Option<String>,
}

/// The view `rules::view` returns for a found record, field for field. The card number stays a
/// [`CardNumber`], so only its redacted form is ever serialised.
impl From<rules::TranView> for TranViewResponse {
    fn from(view: rules::TranView) -> Self {
        Self {
            tran_id: map_field(&view.tran_id),
            card_num: view.card_num,
            type_cd: map_field(&view.type_cd),
            cat_cd: map_field(&view.cat_cd),
            source: map_field(&view.source),
            description: map_field(&view.desc),
            amount: view.amount,
            orig_ts: view.orig_ts,
            proc_ts: view.proc_ts,
            merchant_id: map_field(&view.merchant_id),
            merchant_name: map_field(&view.merchant_name),
            merchant_city: map_field(&view.merchant_city),
            merchant_zip: map_field(&view.merchant_zip),
            message: None,
        }
    }
}

/// A map field as endpoint 12 returns it: all `N` bytes, trailing spaces included.
///
/// The COTRN01C audit fixes each detail value as the bytes the screen shows, padding included
/// (COTRN01C-016 to COTRN01C-028), so the folder's `super::text`, which drops trailing spaces,
/// is not used here. The stored bytes are printable ASCII, so the conversion loses nothing.
fn map_field<const N: usize>(field: &PicX<N>) -> String {
    String::from_utf8_lossy(field.as_bytes()).into_owned()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::{self, Body};
    use axum::extract::Request;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::{Json, Router};
    use chrono::{NaiveDate, NaiveDateTime};
    use domain::CardNumber;
    use domain::copybook::PicX;
    use domain::online::cotrn01c as rules;
    use serde_json::{Value, json};
    use tower::ServiceExt as _;
    use utoipa::{IntoParams, PartialSchema, ToSchema};

    use super::{TranIdPath, TranViewResponse};
    use crate::dto::{FIELD_LENGTH_MESSAGE, INVALID_PATH, ValidPath};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `TRAN-CARD-NUM` of the test view and its redacted form (PIC-024).
    const CARD: &str = "4859452612877065";
    const CARD_REDACTED: &str = "************7065";

    /// The instant `date` at `time` and `micros` microseconds.
    fn at(
        (year, month, day): (i32, u32, u32),
        (hour, minute, second): (u32, u32, u32),
        micros: u32,
    ) -> Result<NaiveDateTime, &'static str> {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_micro_opt(hour, minute, second, micros))
            .ok_or("no such instant")
    }

    fn pic<const N: usize>(text: &str) -> Result<PicX<N>, String> {
        PicX::from_text(text).map_err(|error| format!("{text:?}: {error:?}"))
    }

    /// The view of `dailytran.txt` line 1 as `rules::view` gives it, with a `posttran`
    /// processing timestamp.
    fn view() -> Result<rules::TranView, Box<dyn StdError>> {
        Ok(rules::TranView {
            tran_id: pic("0000000000683580")?,
            card_num: CardNumber::parse(CARD)?,
            type_cd: pic("01")?,
            cat_cd: pic("0001")?,
            source: pic("POS TERM")?,
            desc: pic("Purchase at Abshire-Lowe")?,
            amount: "+00000050.47".to_owned(),
            orig_ts: Some(at((2022, 6, 10), (19, 27, 53), 0)?),
            proc_ts: Some(at((2022, 7, 18), (10, 15, 42), 370_000)?),
            merchant_id: pic("800000000")?,
            merchant_name: pic("Abshire-Lowe")?,
            merchant_city: pic("North Enoshaven")?,
            merchant_zip: pic("72112")?,
        })
    }

    /// The schema of `TranViewResponse` as JSON.
    fn schema() -> Result<Value, serde_json::Error> {
        serde_json::to_value(<TranViewResponse as PartialSchema>::schema())
    }

    // audit: COTRN01C-015
    // audit: COTRN01C-016
    // audit: COTRN01C-018
    // audit: COTRN01C-019
    // audit: COTRN01C-020
    // audit: COTRN01C-021
    // audit: COTRN01C-022
    // audit: COTRN01C-025
    // audit: COTRN01C-026
    // audit: COTRN01C-027
    // audit: COTRN01C-028
    #[test]
    fn every_map_field_keeps_its_full_width_with_no_message() -> TestResult {
        let response = TranViewResponse::from(view()?);
        let widths = [
            (response.tran_id.as_str(), "0000000000683580"),
            (&response.type_cd, "01"),
            (&response.cat_cd, "0001"),
            (&response.source, "POS TERM  "),
            (&response.amount, "+00000050.47"),
            (&response.merchant_id, "800000000"),
            (&response.merchant_zip, "72112     "),
        ];
        for (shown, expected) in widths {
            assert_eq!(shown, expected);
        }
        assert_eq!(
            response.description,
            format!("{:<60}", "Purchase at Abshire-Lowe")
        );
        assert_eq!(response.merchant_name, format!("{:<30}", "Abshire-Lowe"));
        assert_eq!(response.merchant_city, format!("{:<25}", "North Enoshaven"));
        assert_eq!(response.message, None);
        Ok(())
    }

    // audit: COTRN01C-017
    // audit: COTRN01C-023
    // audit: COTRN01C-024
    #[test]
    fn the_body_holds_the_redacted_card_and_typed_timestamps() -> TestResult {
        let body = serde_json::to_value(TranViewResponse::from(view()?))?;
        assert_eq!(
            body,
            json!({
                "tran_id": "0000000000683580",
                "card_num": CARD_REDACTED,
                "type_cd": "01",
                "cat_cd": "0001",
                "source": "POS TERM  ",
                "description": format!("{:<60}", "Purchase at Abshire-Lowe"),
                "amount": "+00000050.47",
                "orig_ts": "2022-06-10T19:27:53",
                "proc_ts": "2022-07-18T10:15:42.370",
                "merchant_id": "800000000",
                "merchant_name": format!("{:<30}", "Abshire-Lowe"),
                "merchant_city": format!("{:<25}", "North Enoshaven"),
                "merchant_zip": "72112     ",
                "message": null,
            })
        );
        let text = body.to_string();
        assert!(!text.contains(CARD), "{text}");
        let debug = format!("{:?}", TranViewResponse::from(view()?));
        assert!(!debug.contains(CARD), "{debug}");
        Ok(())
    }

    // audit: COTRN01C-023
    // audit: COTRN01C-024
    #[test]
    fn blank_timestamps_travel_as_null() -> TestResult {
        let mut blank = view()?;
        blank.orig_ts = None;
        blank.proc_ts = None;
        let body = serde_json::to_value(TranViewResponse::from(blank))?;
        assert_eq!(body.get("orig_ts"), Some(&Value::Null));
        assert_eq!(body.get("proc_ts"), Some(&Value::Null));
        Ok(())
    }

    #[test]
    fn the_schema_is_named_typed_and_redacted() -> TestResult {
        assert_eq!(TranViewResponse::name(), "TranViewResponse");
        let schema = schema()?;
        let properties = schema
            .get("properties")
            .ok_or("the schema has no properties")?;
        for timestamp in ["orig_ts", "proc_ts"] {
            assert_eq!(
                properties.pointer(&format!("/{timestamp}/format")),
                Some(&json!("date-time")),
                "{timestamp}: {properties}"
            );
        }
        assert_eq!(properties.pointer("/card_num/type"), Some(&json!("string")));
        assert_eq!(
            properties.pointer("/card_num/example"),
            Some(&json!("************5740"))
        );
        let text = schema.to_string();
        assert!(!text.contains(CARD), "{text}");
        let required = schema
            .get("required")
            .and_then(Value::as_array)
            .ok_or("the schema lists no required members")?;
        for member in ["tran_id", "card_num", "amount", "merchant_zip"] {
            assert!(required.contains(&json!(member)), "{member}: {required:?}");
        }
        Ok(())
    }

    /// The status and body the route of endpoint 12 gives `uri` once `ValidPath` has extracted
    /// its [`TranIdPath`]: the Tran ID it would pass on, or the rejection.
    async fn through_path(uri: &str) -> Result<(StatusCode, Value), Box<dyn StdError>> {
        let router = Router::new().route(
            "/api/v1/transactions/{tran_id}",
            get(|ValidPath(path): ValidPath<TranIdPath>| async move {
                Json(json!({ "tran_id": path.tran_id }))
            }),
        );
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty())?)
            .await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    // audit: COTRN01-013
    #[tokio::test]
    async fn the_path_tran_id_is_revalidated_against_its_map_length_alone() -> TestResult {
        let passed = |tran_id: &str| (StatusCode::OK, json!({ "tran_id": tran_id }));
        let rejected = |message: &str, field: Option<&str>| {
            let body = match field {
                Some(field) => json!({ "message": message, "field": field }),
                None => json!({ "message": message }),
            };
            (StatusCode::BAD_REQUEST, body)
        };
        let low_values = "%00".repeat(16);
        let spaces_17 = "%20".repeat(17);
        let nuls = "\0".repeat(16);
        for (segment, expected) in [
            // Within the map `LENGTH` the text passes as typed to the program's own edits:
            // sixteen characters, and the blank, low-value, short, leading-space and non-ASCII
            // keys the domain answers itself (COTRN01C-009, COTRN01C-010, COTRN01C-012).
            ("0000000000683580", passed("0000000000683580")),
            ("%20", passed(" ")),
            (low_values.as_str(), passed(&nuls)),
            ("1", passed("1")),
            ("%20000000000000001", passed(" 000000000000001")),
            ("%C3%A9", passed("\u{e9}")),
            // Beyond 16 characters, blank or not: rejected naming the segment, never read.
            (
                "00000000006835801",
                rejected(FIELD_LENGTH_MESSAGE, Some("tran_id")),
            ),
            (
                spaces_17.as_str(),
                rejected(FIELD_LENGTH_MESSAGE, Some("tran_id")),
            ),
            // A segment that is not UTF-8 does not deserialise.
            ("%FF", rejected(INVALID_PATH, None)),
        ] {
            let uri = format!("/api/v1/transactions/{segment}");
            assert_eq!(through_path(&uri).await?, expected, "{uri}");
        }
        Ok(())
    }

    // audit: COTRN01-013
    #[test]
    fn the_path_parameter_publishes_its_map_length() -> TestResult {
        let parameters = serde_json::to_value(TranIdPath::into_params(|| None))?;
        assert_eq!(parameters.as_array().map(Vec::len), Some(1), "{parameters}");
        for (pointer, expected) in [
            ("/0/name", json!("tran_id")),
            ("/0/in", json!("path")),
            ("/0/required", json!(true)),
            ("/0/schema/type", json!("string")),
            ("/0/schema/maxLength", json!(16)),
        ] {
            assert_eq!(parameters.pointer(pointer), Some(&expected), "{parameters}");
        }
        Ok(())
    }
}
