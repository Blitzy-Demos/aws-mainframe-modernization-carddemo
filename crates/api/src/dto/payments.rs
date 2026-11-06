//! CB00 bill payment (`COBIL00C`, map `COBIL00`): the body of
//! `POST /payments`, its response and the BMS fields its errors name.
//!
//! The request carries the two input fields of `COBIL0AI` as entered: the
//! account ID (`ACTIDINI X(11)`) and the `Y`/`N` confirmation (`CONFIRMI
//! X(1)`) [app/cpy-bms/COBIL00.CPY:L60]. Every edit, the account read and the
//! balance arithmetic belong to `carddemo_domain::rules::bill_payment`; this
//! module only bounds the field lengths and converts at the HTTP boundary.
//!
//! The same response shape answers both successful outcomes of
//! `PROCESS-ENTER-KEY` [app/cbl/COBIL00C.cbl:L154-244]:
//!
//! - the blank-confirmation prompt, 200 with no write: the `CURBAL` balance
//!   and `Confirm to make a bill payment...` (V-COBIL00C-008);
//! - the confirmed payment, 201: the balance after the payment, the new
//!   TRAN-ID and `Payment successful.  Your Transaction ID is <tran-id>.`
//!   (P-COBIL00C-016).
//!
//! Every failure, the write-persistence outcomes of E-19 included, is an
//! [`ApiError`] with the error body, never this response.

use carddemo_domain::numeric::{AccountId, Money};
use carddemo_domain::rules::Confirm;
use carddemo_domain::rules::bill_payment::{
    BillPaymentInput, FIELD_ACTIDIN, FIELD_CONFIRM, PaymentSuccess,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::{BmsField, id_string, money, reject_invalid};
use crate::error::ApiError;

/// The `message` of a request whose fields exceed their BMS lengths: none.
///
/// `ACTIDIN` and `CONFIRM` are `LENGTH=11` and `LENGTH=1` on the map
/// [app/bms/COBIL00.bms:L85-89] [app/bms/COBIL00.bms:L115-119], so the 3270
/// screen never delivers a longer entry and no `V-COBIL00C` row records a
/// text for one. Such a request has no COBOL counterpart (E-49): it is
/// rejected with 400 `Invalid`, the over-long fields in `fields`, and no
/// message, rather than being cut to the field width or given the text of an
/// unrelated edit.
const OVER_LENGTH_MESSAGE: &str = "";

/// The body of `POST /payments`: one ENTER on the `COBIL0A` screen.
///
/// Both fields are taken as entered; a missing field is a blank one, which
/// the rule answers with its audited message (`Acct ID can NOT be empty...`,
/// V-COBIL00C-001) or treats as an unconfirmed payment (V-COBIL00C-008).
#[derive(Debug, Clone, Default, Deserialize, ToSchema, Validate)]
pub struct PaymentRequest {
    /// `ACTIDIN` (`ACTIDINI X(11)`): the account to pay, 11 digits such as
    /// `00000000001`.
    #[serde(default)]
    #[schema(max_length = 11)]
    #[validate(length(max = 11))]
    pub account_id: Option<String>,
    /// `CONFIRM` (`CONFIRMI X(1)`): `Y` or `y` pays, `N` or `n` cancels,
    /// blank or missing shows the balance and asks for confirmation.
    #[serde(default)]
    #[schema(max_length = 1)]
    #[validate(length(max = 1))]
    pub confirm: Option<String>,
}

impl PaymentRequest {
    /// The BMS length edit: `account_id` at most 11 characters and `confirm`
    /// at most 1, the widths of `ACTIDINI` and `CONFIRMI`.
    ///
    /// # Errors
    ///
    /// 400 `Invalid` with no message (`OVER_LENGTH_MESSAGE`) and one
    /// `fields` entry per over-long field, `ACTIDIN` before `CONFIRM`.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, bms_field, OVER_LENGTH_MESSAGE))
    }

    /// COBIL00C PROCESS-ENTER-KEY — V-COBIL00C-001 to V-COBIL00C-003.
    ///
    /// The rule input: the account ID unchanged, and the confirmation
    /// classified as `EVALUATE CONFIRMI OF COBIL0AI` classifies it
    /// [app/cbl/COBIL00C.cbl:L173-191], through [`Confirm::parse`].
    #[must_use]
    pub fn into_input(self) -> BillPaymentInput {
        BillPaymentInput {
            confirm: Confirm::parse(self.confirm.as_deref()),
            account_id: self.account_id,
        }
    }
}

/// The request field a failed length check names, by its JSON name.
fn bms_field(name: &str) -> Option<BillPaymentField> {
    match name {
        "account_id" => Some(BillPaymentField::ActIdIn),
        "confirm" => Some(BillPaymentField::Confirm),
        _ => None,
    }
}

/// The answer of `POST /payments` that is not an error: the confirmation
/// prompt (200) or the completed payment (201).
///
/// `tran_id` is present only for a completed payment. It is left out of the
/// prompt rather than sent as `null`, so the schema declares it an optional,
/// non-nullable string and the document stays the same from run to run.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PaymentResponse {
    /// The account, as the 11 zero-padded digits of `ACCT-ID PIC 9(11)`.
    pub account_id: String,
    /// `ACCT-CURR-BAL PIC S9(10)V99` as a scale-2 decimal string such as
    /// `"194.00"`: the `CURBAL` balance with the prompt, the balance after the
    /// payment once it is made.
    #[serde(with = "rust_decimal::serde::str")]
    pub current_balance: Decimal,
    /// The 16-digit TRAN-ID of the payment transaction; absent with the
    /// prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub tran_id: Option<String>,
    /// The message line: `Confirm to make a bill payment...` with the prompt,
    /// `Payment successful.  Your Transaction ID is <tran-id>.` with the
    /// payment.
    pub message: String,
}

impl PaymentResponse {
    /// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-007; V-COBIL00C-008.
    ///
    /// The blank-confirmation answer [app/cbl/COBIL00C.cbl:L193-194]
    /// [app/cbl/COBIL00C.cbl:L236-242]: the balance moved to `CURBAL` and the
    /// prompt `message` of `BillPaymentStep::ConfirmPrompt`, with no TRAN-ID,
    /// since nothing is written.
    #[must_use]
    pub fn confirm_prompt(
        account_id: AccountId,
        current_balance: Money<10>,
        message: String,
    ) -> Self {
        Self {
            account_id: id_string(&account_id),
            current_balance: money(current_balance),
            tran_id: None,
            message,
        }
    }

    /// COBIL00C WRITE-TRANSACT-FILE, UPDATE-ACCTDAT-FILE — P-COBIL00C-016,
    /// P-COBIL00C-022.
    ///
    /// The completed payment [app/cbl/COBIL00C.cbl:L233-235]
    /// [app/cbl/COBIL00C.cbl:L527-531]: the account, its rewritten balance,
    /// the TRAN-ID written and the success message built with it.
    ///
    /// The COBOL blanks `CURBAL` once the write succeeds
    /// (`INITIALIZE-ALL-FIELDS` [app/cbl/COBIL00C.cbl:L560-566]); the response
    /// carries the rewritten balance instead, so the screen can show the
    /// account's state after the payment without a second request.
    #[must_use]
    pub fn paid(success: &PaymentSuccess) -> Self {
        Self {
            account_id: id_string(&success.account_id),
            current_balance: money(success.new_balance),
            tran_id: Some(id_string(&success.tran_id)),
            message: success.message.clone(),
        }
    }
}

/// The BMS fields of `COBIL0A` that the errors of `POST /payments` name in
/// `fields[].field`: the distinct `BMS field` values of the `V-COBIL00C` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum BillPaymentField {
    /// `ACTIDIN`, the account ID: every `V-COBIL00C` row but two
    /// (`MOVE -1 TO ACTIDINL`).
    #[serde(rename = "ACTIDIN")]
    ActIdIn,
    /// `CONFIRM`, the confirmation: the invalid value (V-COBIL00C-003) and
    /// the prompt (V-COBIL00C-008) (`MOVE -1 TO CONFIRML`).
    #[serde(rename = "CONFIRM")]
    Confirm,
}

impl BillPaymentField {
    /// The BMS field name, the `fields[].field` value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ActIdIn => FIELD_ACTIDIN,
            Self::Confirm => FIELD_CONFIRM,
        }
    }
}

impl BmsField for BillPaymentField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The lowest `Order` of the field's `V-COBIL00C` rows: `ACTIDIN` 1
    /// (V-COBIL00C-001), `CONFIRM` 2 (V-COBIL00C-003), the order in which
    /// `PROCESS-ENTER-KEY` edits the two fields.
    fn rank(self) -> u16 {
        match self {
            Self::ActIdIn => 1,
            Self::Confirm => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages::{CONFIRM_TO_MAKE_A_BILL_PAYMENT, payment_successful_message};
    use carddemo_domain::numeric::{AccountId, Money, TranId};
    use carddemo_domain::rules::Confirm;
    use carddemo_domain::rules::bill_payment::{BillPaymentInput, PaymentSuccess};
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    use super::{BillPaymentField, BmsField, PaymentRequest, PaymentResponse};

    /// The first `acctdata.txt` account (P-CVACT01Y-015).
    const ACCOUNT_ENTRY: &str = "00000000001";

    fn request(account_id: Option<&str>, confirm: Option<&str>) -> PaymentRequest {
        PaymentRequest {
            account_id: account_id.map(str::to_owned),
            confirm: confirm.map(str::to_owned),
        }
    }

    fn account(value: u64) -> AccountId {
        AccountId::new(value).expect("an 11-digit account number")
    }

    #[test]
    fn check_rejects_over_long_fields_with_their_bms_names() {
        let twelve = "000000000012";
        // (account_id, confirm, BMS fields in the body)
        let cases: [(Option<&str>, Option<&str>, &[&str]); 3] = [
            (Some(twelve), Some("Y"), &["ACTIDIN"]),
            (Some(ACCOUNT_ENTRY), Some("YY"), &["CONFIRM"]),
            (Some(twelve), Some("YY"), &["ACTIDIN", "CONFIRM"]),
        ];
        for (account_id, confirm, fields) in cases {
            let error = request(account_id, confirm)
                .check()
                .expect_err("a field exceeds its BMS length");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{fields:?}");
            let entries: Vec<Value> = fields
                .iter()
                .map(|field| json!({"field": field, "message": ""}))
                .collect();
            assert_eq!(
                serde_json::to_value(error.body()).expect("serializes"),
                json!({
                    "code": ErrorKind::Invalid.code(),
                    "message": "",
                    "fields": entries,
                }),
                "{fields:?}"
            );
        }
    }

    #[test]
    fn check_accepts_fields_within_their_bms_lengths() {
        let cases = [
            request(None, None),
            request(Some(""), Some("")),
            request(Some(ACCOUNT_ENTRY), Some("Y")),
            request(Some("0000000000A"), Some("X")),
            // Eleven characters, not eleven bytes: the edit counts what was
            // entered.
            request(Some("ééééééééééé"), Some("é")),
        ];
        for body in cases {
            assert!(body.check().is_ok(), "{body:?}");
        }
    }

    #[test]
    fn into_input_keeps_the_account_and_classifies_the_confirmation() {
        let cases = [
            (None, Confirm::Blank),
            (Some(""), Confirm::Blank),
            (Some(" "), Confirm::Blank),
            (Some("\0"), Confirm::Blank),
            (Some("Y"), Confirm::Yes),
            (Some("y"), Confirm::Yes),
            (Some("N"), Confirm::No),
            (Some("n"), Confirm::No),
            (Some("X"), Confirm::Other("X".to_owned())),
        ];
        for (confirm, expected) in cases {
            for account_id in [None, Some(" 1"), Some(ACCOUNT_ENTRY)] {
                assert_eq!(
                    request(account_id, confirm).into_input(),
                    BillPaymentInput {
                        account_id: account_id.map(str::to_owned),
                        confirm: expected.clone(),
                    },
                    "{account_id:?} {confirm:?}"
                );
            }
        }
    }

    #[test]
    fn request_deserializes_missing_fields_as_blank() {
        let empty: PaymentRequest = serde_json::from_str("{}").expect("deserializes");
        assert_eq!(empty.account_id, None);
        assert_eq!(empty.confirm, None);

        let full: PaymentRequest =
            serde_json::from_value(json!({"account_id": ACCOUNT_ENTRY, "confirm": "y"}))
                .expect("deserializes");
        assert_eq!(full.account_id.as_deref(), Some(ACCOUNT_ENTRY));
        assert_eq!(full.confirm.as_deref(), Some("y"));
    }

    #[test]
    fn confirm_prompt_serializes_the_balance_without_a_tran_id() {
        let balance = Money::<10>::from_cents(123_450).expect("fits S9(10)V99");
        let response = PaymentResponse::confirm_prompt(
            account(1),
            balance,
            CONFIRM_TO_MAKE_A_BILL_PAYMENT.to_owned(),
        );
        assert_eq!(
            serde_json::to_string(&response).expect("serializes"),
            format!(
                r#"{{"account_id":"00000000001","current_balance":"1234.50","message":"{CONFIRM_TO_MAKE_A_BILL_PAYMENT}"}}"#
            )
        );
    }

    #[test]
    fn paid_carries_the_tran_id_new_balance_and_message() {
        let tran_id = "0000000000000043";
        let success = PaymentSuccess {
            tran_id: TranId::move_from(tran_id),
            message: payment_successful_message(tran_id),
            new_balance: Money::<10>::ZERO,
            account_id: account(99_999_999_999),
        };
        let response = PaymentResponse::paid(&success);
        assert_eq!(response.tran_id.as_deref().map(str::len), Some(16));
        assert_eq!(
            serde_json::to_value(&response).expect("serializes"),
            json!({
                "account_id": "99999999999",
                "current_balance": "0.00",
                "tran_id": tran_id,
                "message": payment_successful_message(tran_id),
            })
        );
    }

    #[test]
    fn field_names_match_on_the_wire_and_in_rank_order() {
        let fields = [BillPaymentField::ActIdIn, BillPaymentField::Confirm];
        for field in fields {
            assert_eq!(
                serde_json::to_value(field).expect("serializes"),
                json!(field.as_str())
            );
            let parsed: BillPaymentField =
                serde_json::from_value(json!(field.as_str())).expect("deserializes");
            assert_eq!(parsed, field);
            assert_eq!(BmsField::as_str(field), field.as_str());
        }
        assert_eq!(fields.map(BillPaymentField::as_str), ["ACTIDIN", "CONFIRM"]);
        assert!(BmsField::rank(fields[0]) < BmsField::rank(fields[1]));
    }

    #[test]
    fn schemas_declare_the_lengths_the_field_enum_and_an_optional_tran_id() {
        let request = serde_json::to_value(PaymentRequest::schema()).expect("serializes");
        assert_eq!(request["properties"]["account_id"]["maxLength"], json!(11));
        assert_eq!(request["properties"]["confirm"]["maxLength"], json!(1));
        assert!(request.get("required").is_none(), "{request}");

        let response = serde_json::to_value(PaymentResponse::schema()).expect("serializes");
        assert_eq!(
            response["required"],
            json!(["account_id", "current_balance", "message"])
        );
        assert_eq!(response["properties"]["tran_id"]["type"], json!("string"));
        assert_eq!(
            response["properties"]["current_balance"]["type"],
            json!("string")
        );

        let field = serde_json::to_value(BillPaymentField::schema()).expect("serializes");
        assert_eq!(field["enum"], json!(["ACTIDIN", "CONFIRM"]));
    }
}
