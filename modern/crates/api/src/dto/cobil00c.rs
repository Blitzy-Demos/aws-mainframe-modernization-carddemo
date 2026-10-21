//! Endpoint 15, `POST /api/v1/bill-payments`: the contract of COBIL00C (transaction `CB00`, map
//! `COBIL0A`; AAP 0.3.4.1).
//!
//! ENTER sends the two inputs of the map. A blank confirmation shows the balance and asks for
//! confirmation (COBIL00C-011, COBIL00C-019), `Y` or `y` pays (COBIL00C-009, COBIL00C-028) and
//! `N` or `n` clears the screen (COBIL00C-010). F3 and F4 send no request.
//!
//! Every outcome is decided by `domain::online::cobil00c` and carried out by
//! `api::services::cobil00c`; this module only carries the result. The balance arrives as the
//! 14-character text `rules::balance_display` edits with `PIC +9999999999.99` (ARITH-019), and
//! the payment arithmetic, the balance stored into `TRAN-AMT PIC S9(09)V99` (ARITH-020,
//! ARITH-021, E11), never reaches this module. No member here is a number.
//!
//! A failure, from `Acct ID can NOT be empty...` (COBIL00C-007) to `Unable to Update Account...`
//! (COBIL00C-033), is a `rules::BillPayFailure`, answered as a `ScreenError` by
//! `api::error`; it is never a member of [`BillPaymentResult`].

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use domain::online::cobil00c as rules;

use super::{digits, text};

/// The two inputs of `COBIL0A`, each the raw text of its map field.
///
/// Only the map `LENGTH` is checked here, the server half of the `both` rows `COBIL00-013` and
/// `COBIL00-020` of `modern/audit/screens.md`. An absent member is the empty text, which the
/// program's own edit answers with `Acct ID can NOT be empty...` (COBIL00C-007). The values are
/// neither trimmed nor upper-cased: COBIL00C reads them as typed.
// The schema name `BillPaymentRequest` is the frontend's
// `components["schemas"]["BillPaymentRequest"]` (Generated API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({"acct_id": "00000000001", "confirm": ""}))]
pub struct BillPaymentRequest {
    /// `ACTIDIN`, the account ID, at most 11 characters (`COBIL00-013`). Eleven digits name an
    /// account; a blank value is refused (COBIL00C-007, COBIL00C-008).
    #[serde(default)]
    #[validate(length(max = 11))]
    #[schema(max_length = 11)]
    pub acct_id: String,
    /// `CONFIRM`, at most 1 character (`COBIL00-020`): blank shows the balance, `Y` or `y` pays,
    /// `N` or `n` clears; any other value is refused (COBIL00C-009 to COBIL00C-012).
    #[serde(default)]
    #[validate(length(max = 1))]
    #[schema(max_length = 1)]
    pub confirm: String,
}

/// The request as the inputs `rules::edit` and its parts read, borrowed unchanged.
impl<'a> From<&'a BillPaymentRequest> for rules::BillPayInput<'a> {
    fn from(request: &'a BillPaymentRequest) -> Self {
        Self {
            acct_id: &request.acct_id,
            confirm: &request.confirm,
        }
    }
}

/// The screen a 200 of endpoint 15 shows: the account, the balance, the message row and the new
/// transaction ID.
///
/// `cleared` is true where the program ran `INITIALIZE-ALL-FIELDS` before its last send: after
/// `N` (COBIL00C-010) and after a written payment (COBIL00C-028). The screen then empties its
/// inputs and its balance, and shows `message`.
// The schema name `BillPaymentResult` is the frontend's
// `components["schemas"]["BillPaymentResult"]` (Generated API Types Only). An absent optional
// member is omitted from the body, as in `ScreenError`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "acct_id": "00000000001",
    "current_balance": "+0000000843.00",
    "message": "Confirm to make a bill payment...",
    "cleared": false
}))]
pub struct BillPaymentResult {
    /// `ACTIDIN` as shown: the 11 digits of the account whose balance is shown, and the empty
    /// text where the screen is cleared (COBIL00C-010, COBIL00C-019, COBIL00C-028).
    pub acct_id: String,
    /// `CURBAL`, the balance as `PIC +9999999999.99` edits it, 14 characters such as
    /// `+0000000843.00`; present only with the confirmation prompt (COBIL00C-017, COBIL00C-019,
    /// ARITH-019).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(max_length = 14)]
    pub current_balance: Option<String>,
    /// The message row: `Confirm to make a bill payment...` with the balance (COBIL00C-019),
    /// `Payment successful.  Your Transaction ID is <id>.` after a payment (COBIL00C-028), absent
    /// after `N` (COBIL00C-010).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `TRAN-ID` of the payment just written, 16 digits; present only after a payment
    /// (COBIL00C-023, COBIL00C-028).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(max_length = 16)]
    pub new_tran_id: Option<String>,
    /// Whether the screen empties its inputs and balance: after `N` (COBIL00C-010) and after a
    /// payment (COBIL00C-028).
    pub cleared: bool,
}

/// The outcome the service returns, field by field; every text is the domain's.
impl From<rules::BillPayOutcome> for BillPaymentResult {
    fn from(outcome: rules::BillPayOutcome) -> Self {
        let message = outcome.message();
        match outcome {
            // `N` and a written payment both run `INITIALIZE-ALL-FIELDS`, which empties
            // `ACTIDIN`, `CURBAL` and `CONFIRM`, so both answer a cleared screen.
            rules::BillPayOutcome::Cleared => Self {
                acct_id: String::new(),
                current_balance: None,
                message,
                new_tran_id: None,
                cleared: true,
            },
            rules::BillPayOutcome::BalanceShown { acct_id, balance } => Self {
                acct_id: digits(acct_id),
                current_balance: Some(balance),
                message,
                new_tran_id: None,
                cleared: false,
            },
            rules::BillPayOutcome::Paid { tran_id } => Self {
                acct_id: String::new(),
                current_balance: None,
                message,
                new_tran_id: Some(text(&tran_id)),
                cleared: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::http::StatusCode;
    use domain::copybook::{PicU, PicX};
    use validator::Validate;

    use super::super::{FIELD_LENGTH_MESSAGE, validation_error};
    use super::{BillPaymentRequest, BillPaymentResult, rules};

    type TestResult = Result<(), Box<dyn StdError>>;

    // The audited literals, written out apart from the domain's constants.
    const PROMPT: &str = "Confirm to make a bill payment...";
    const PAID: &str = "Payment successful.  Your Transaction ID is 0000000000000001.";

    fn request(acct_id: &str, confirm: &str) -> BillPaymentRequest {
        BillPaymentRequest {
            acct_id: acct_id.to_owned(),
            confirm: confirm.to_owned(),
        }
    }

    #[test]
    fn absent_members_are_empty_text() -> TestResult {
        // audit: COBIL00-013
        // audit: COBIL00-020
        let empty: BillPaymentRequest = serde_json::from_str("{}")?;
        assert_eq!(empty, request("", ""));
        assert!(empty.validate().is_ok());
        Ok(())
    }

    #[test]
    fn inputs_longer_than_the_map_field_are_a_400_naming_the_field() -> TestResult {
        let cases = [
            // audit: COBIL00-013
            (request("000000000011", ""), "acct_id"),
            // audit: COBIL00-020
            (request("00000000001", "YY"), "confirm"),
        ];
        for (input, field) in cases {
            let errors = input.validate().err().ok_or("over-length input passed")?;
            let rejection = validation_error(&errors);
            assert_eq!(
                (rejection.status(), rejection.message(), rejection.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(field))
            );
        }
        for input in [request("00000000001", "Y"), request("ABCDEFGHIJK", "x")] {
            assert!(input.validate().is_ok(), "{input:?}");
        }
        Ok(())
    }

    #[test]
    fn the_request_reaches_the_domain_edits_unchanged() {
        let typed = request(" 0000000001", "n");
        let input = rules::BillPayInput::from(&typed);
        assert_eq!((input.acct_id, input.confirm), (" 0000000001", "n"));
        // audit: COBIL00C-010
        assert_eq!(
            rules::edit(&rules::BillPayInput::from(&request("00000000001", "N"))),
            Ok(rules::EnterAction::Clear)
        );
    }

    #[test]
    fn each_outcome_maps_to_its_screen() -> TestResult {
        let cases = [
            (
                // audit: COBIL00C-010
                rules::BillPayOutcome::Cleared,
                BillPaymentResult {
                    acct_id: String::new(),
                    current_balance: None,
                    message: None,
                    new_tran_id: None,
                    cleared: true,
                },
            ),
            (
                // audit: COBIL00C-019
                rules::BillPayOutcome::BalanceShown {
                    acct_id: PicU::new(1)?,
                    balance: "+0000000843.00".to_owned(),
                },
                BillPaymentResult {
                    acct_id: "00000000001".to_owned(),
                    current_balance: Some("+0000000843.00".to_owned()),
                    message: Some(PROMPT.to_owned()),
                    new_tran_id: None,
                    cleared: false,
                },
            ),
            (
                // audit: COBIL00C-028
                rules::BillPayOutcome::Paid {
                    tran_id: PicX::from_array(*b"0000000000000001"),
                },
                BillPaymentResult {
                    acct_id: String::new(),
                    current_balance: None,
                    message: Some(PAID.to_owned()),
                    new_tran_id: Some("0000000000000001".to_owned()),
                    cleared: true,
                },
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(BillPaymentResult::from(outcome), expected);
        }
        Ok(())
    }

    #[test]
    fn absent_members_are_omitted_from_the_body_and_read_back() -> TestResult {
        let cleared = BillPaymentResult::from(rules::BillPayOutcome::Cleared);
        let body = serde_json::to_value(&cleared)?;
        assert_eq!(body, serde_json::json!({"acct_id": "", "cleared": true}));
        assert_eq!(serde_json::from_value::<BillPaymentResult>(body)?, cleared);

        let shown = BillPaymentResult::from(rules::BillPayOutcome::BalanceShown {
            acct_id: PicU::new(1)?,
            balance: "-0000000025.00".to_owned(),
        });
        assert_eq!(
            serde_json::to_value(&shown)?,
            serde_json::json!({
                "acct_id": "00000000001",
                "current_balance": "-0000000025.00",
                "message": PROMPT,
                "cleared": false
            })
        );
        Ok(())
    }
}
