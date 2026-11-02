//! The wire types of the admin reward-rule endpoints: `GET`, `POST`, `PUT` and
//! `DELETE /admin/reward-rules` (request Rewards system, admin endpoints).
//!
//! Rewards have no COBOL program and no BMS map: the README lists DB2 Rewards
//! only as a roadmap item [README.md:L357-369]. A rule's codes are the fixture
//! codes of `app/data/ASCII/trantype.txt` and `app/data/ASCII/trancatg.txt`,
//! such as type `01` Purchase with category `0001` Regular Sales Draft, and its
//! amount bounds compare with `TRAN-AMT PIC S9(09)V99` [app/cpy/CVTRA05Y.cpy:L10].
//!
//! Two of the `crate::dto` conventions do not apply, because both serve BMS
//! maps:
//!
//! - The required fields of [`RewardRuleRequest`] are plain `String`s, not
//!   `Option<String>`, so the `OpenAPI` document lists them as required and a
//!   body without one is rejected by `crate::error::ApiJson` (400 `INVALID`).
//!   No COBOL rule exists to answer a blank field with a message of its own.
//! - `fields[].field` carries the request's own JSON names, which
//!   [`RewardRuleField`] lists.
//!
//! Decimals and instants travel as JSON strings, never as floating-point
//! numbers. [`RewardRuleRequest::into_draft`] only converts the text into the
//! domain's types and enforces the text patterns the `OpenAPI` document
//! declares. The rule checks themselves (a non-negative rate, the minimum not
//! above the maximum, `active_to` not before `active_from`) belong to
//! `carddemo_domain::rewards::validate_rule`, which the handler calls with the
//! draft, so each keeps its one message.

use carddemo_domain::entities::RewardRule;
use carddemo_domain::numeric::{Money, TranCategoryCode, TranTypeCode};
use carddemo_domain::rewards::RuleDraft;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::dto::{BmsField, id_string, money};
use crate::error::ApiError;

/// The fraction digits an amount bound may be written with: the `V99` of
/// `S9(09)V99`.
const AMOUNT_SCALE: u32 = 2;

/// The rejection reason of a decimal field whose text is not
/// `-?[0-9]+(.[0-9]+)?`.
const NOT_A_DECIMAL: &str = "value is not a decimal number";

/// The rejection reason of a type code holding `U+0000`, which the
/// `reward_rules.transaction_type_cd` text column cannot store.
const HAS_NUL: &str = "value has a NUL character (U+0000)";

/// The body of `POST /admin/reward-rules` and `PUT /admin/reward-rules/{id}`:
/// one reward rule as the administrator entered it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RewardRuleRequest {
    /// The transaction type code the rule matches (`TRAN-TYPE-CD PIC X(02)`),
    /// a fixture type code such as `01` Purchase: at most 2 characters from
    /// `U+0001` to `U+00FF`, the ISO 8859-1 characters its column stores.
    #[schema(max_length = 2, pattern = r"^[\x01-\xFF]*$", example = "01")]
    pub transaction_type_cd: String,
    /// The transaction category code the rule matches
    /// (`TRAN-CAT-CD PIC 9(04)`), 1 to 4 digits, such as `0001` Regular Sales
    /// Draft.
    #[schema(pattern = "^[0-9]{1,4}$", example = "0001")]
    pub transaction_category_cd: String,
    /// Points per currency unit, a decimal string. The rule check accepts at
    /// most 4 fraction digits and values below 100000 (`NUMERIC(9,4)`).
    // The pattern admits a leading `-` so that a negative rate reaches
    // `validate_rule` and is answered with its own message.
    #[schema(pattern = r"^-?[0-9]+(\.[0-9]+)?$", example = "1.0000")]
    pub reward_rate: String,
    /// The inclusive lower bound of a qualifying amount, a decimal string with
    /// at most 2 fraction digits (`S9(09)V99`). Absent or `null` is
    /// unbounded.
    #[serde(default)]
    #[schema(pattern = r"^-?[0-9]+(\.[0-9]{1,2})?$", example = "10.00")]
    pub min_transaction_amount: Option<String>,
    /// The inclusive upper bound of a qualifying amount, a decimal string with
    /// at most 2 fraction digits (`S9(09)V99`). Absent or `null` is
    /// unbounded.
    #[serde(default)]
    #[schema(pattern = r"^-?[0-9]+(\.[0-9]{1,2})?$", example = "500.00")]
    pub max_transaction_amount: Option<String>,
    /// The first instant the rule is active, inclusive, as an RFC 3339
    /// date-time.
    #[schema(format = DateTime, example = "2022-01-01T00:00:00Z")]
    pub active_from: String,
    /// The end of the active window, exclusive, as an RFC 3339 date-time.
    /// Absent or `null` leaves the rule open-ended.
    #[serde(default)]
    #[schema(format = DateTime, example = "2030-01-01T00:00:00Z")]
    pub active_to: Option<String>,
}

impl RewardRuleRequest {
    /// The draft that `carddemo_domain::rewards::validate_rule` checks: each
    /// field converted to its domain type, and nothing else checked.
    ///
    /// `reward_rate` keeps its sign and every digit as entered, so a negative
    /// rate and a rate beyond `NUMERIC(9,4)` reach the rule check, which
    /// answers them with field `reward_rate`. Instants with an offset are
    /// converted to UTC.
    ///
    /// # Errors
    ///
    /// [`ApiError::rejection`] (400 `INVALID`, no `fields`) for the first
    /// field, in declaration order, whose text does not convert. The message
    /// is the field's JSON name, `": "` and the reason:
    ///
    /// - `transaction_type_cd`: a NUL character (`U+0000`), checked first;
    ///   more than 2 characters; or a character above `U+00FF`.
    /// - `transaction_category_cd`: not 1 to 4 ASCII digits.
    /// - `reward_rate`: not `-?[0-9]+(.[0-9]+)?`, or more digits than a
    ///   `Decimal` holds.
    /// - `min_transaction_amount`, `max_transaction_amount`: not
    ///   `-?[0-9]+(.[0-9]{1,2})?`, or a magnitude of 10^9 or more.
    /// - `active_from`, `active_to`: not an RFC 3339 date-time.
    pub fn into_draft(self) -> Result<RuleDraft, ApiError> {
        let Self {
            transaction_type_cd,
            transaction_category_cd,
            reward_rate,
            min_transaction_amount,
            max_transaction_amount,
            active_from,
            active_to,
        } = self;
        let transaction_type_cd = type_code("transaction_type_cd", &transaction_type_cd)?;
        let transaction_category_cd = transaction_category_cd
            .parse::<TranCategoryCode>()
            .map_err(|err| rejected("transaction_category_cd", err))?;
        let reward_rate = decimal("reward_rate", &reward_rate)?;
        let min_transaction_amount =
            optional_amount("min_transaction_amount", min_transaction_amount.as_deref())?;
        let max_transaction_amount =
            optional_amount("max_transaction_amount", max_transaction_amount.as_deref())?;
        let active_from = instant("active_from", &active_from)?;
        let active_to = active_to
            .as_deref()
            .map(|text| instant("active_to", text))
            .transpose()?;
        Ok(RuleDraft {
            transaction_type_cd,
            transaction_category_cd,
            reward_rate,
            min_transaction_amount,
            max_transaction_amount,
            active_from,
            active_to,
        })
    }
}

/// One stored reward rule: every column of a `reward_rules` row, as the list,
/// create, update and soft-delete endpoints answer it.
///
/// The list returns active and soft-deleted rules alike, ordered by
/// `rule_id`; a soft-deleted rule shows the `active_to` its delete set.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RewardRuleResponse {
    /// The rule identity, a decimal integer string: the `{id}` of
    /// `PUT` and `DELETE /admin/reward-rules/{id}`.
    #[schema(pattern = "^[0-9]+$", example = "1")]
    pub rule_id: String,
    /// The transaction type code the rule matches, without trailing spaces.
    #[schema(example = "01")]
    pub transaction_type_cd: String,
    /// The transaction category code the rule matches, zero-padded to the
    /// four digits of `TRAN-CAT-CD PIC 9(04)`.
    #[schema(pattern = "^[0-9]{4}$", example = "0001")]
    pub transaction_category_cd: String,
    /// Points per currency unit, a decimal string with exactly 4 fraction
    /// digits (`NUMERIC(9,4)`).
    #[serde(with = "rust_decimal::serde::str")]
    #[schema(example = "1.0000")]
    pub reward_rate: Decimal,
    /// The inclusive lower amount bound, a decimal string with exactly 2
    /// fraction digits; `null` is unbounded.
    // Required although nullable here and on the fields below: the key is
    // always serialized, as `null` when absent, so the published schema says
    // so.
    #[serde(with = "rust_decimal::serde::str_option")]
    #[schema(required = true, example = "10.00")]
    pub min_transaction_amount: Option<Decimal>,
    /// The inclusive upper amount bound, a decimal string with exactly 2
    /// fraction digits; `null` is unbounded.
    #[serde(with = "rust_decimal::serde::str_option")]
    #[schema(required = true, example = "500.00")]
    pub max_transaction_amount: Option<Decimal>,
    /// The first instant the rule is active, inclusive, in RFC 3339 UTC.
    pub active_from: DateTime<Utc>,
    /// The end of the active window, exclusive, in RFC 3339 UTC; `null` for an
    /// open-ended rule.
    #[schema(required = true)]
    pub active_to: Option<DateTime<Utc>>,
}

impl From<&RewardRule> for RewardRuleResponse {
    fn from(rule: &RewardRule) -> Self {
        Self {
            rule_id: id_string(&rule.rule_id),
            transaction_type_cd: id_string(&rule.transaction_type_cd),
            transaction_category_cd: id_string(&rule.transaction_category_cd),
            reward_rate: rule.reward_rate.to_decimal(),
            min_transaction_amount: rule.min_transaction_amount.map(money),
            max_transaction_amount: rule.max_transaction_amount.map(money),
            active_from: rule.active_from,
            active_to: rule.active_to,
        }
    }
}

/// A field a reward-rule rejection names in `fields[].field`: exactly the
/// fields `carddemo_domain::rewards::validate_rule` reports, under the
/// request's own JSON names.
// No path refers to the enum, so `crate::openapi` registers it for the SPA's
// field map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum RewardRuleField {
    /// `reward_rate`: a negative rate, or one `NUMERIC(9,4)` cannot hold.
    #[serde(rename = "reward_rate")]
    RewardRate,
    /// `min_transaction_amount`: a minimum above the maximum.
    #[serde(rename = "min_transaction_amount")]
    MinTransactionAmount,
    /// `active_to`: an end before the start.
    #[serde(rename = "active_to")]
    ActiveTo,
}

impl RewardRuleField {
    /// The `fields[].field` value: the request field's JSON name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RewardRate => "reward_rate",
            Self::MinTransactionAmount => "min_transaction_amount",
            Self::ActiveTo => "active_to",
        }
    }
}

/// The reward fields have no BMS map; the rank is the order in which
/// `validate_rule` runs its checks and lists the failing fields.
impl BmsField for RewardRuleField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    fn rank(self) -> u16 {
        match self {
            Self::RewardRate => 1,
            Self::MinTransactionAmount => 2,
            Self::ActiveTo => 3,
        }
    }
}

/// The 400 rejection of request field `field`, naming it before `reason`.
fn rejected(field: &str, reason: impl std::fmt::Display) -> ApiError {
    ApiError::rejection(format!("{field}: {reason}"))
}

/// The transaction type code written as `text` in request field `field`.
///
/// A NUL is rejected before the code is built: the code would hold it as byte
/// `0x00`, which the type's own conversion admits but no `PostgreSQL` text
/// value can carry, so the insert or update would fail with a 500. The
/// published pattern `^[\x01-\xFF]*$` excludes it likewise.
fn type_code(field: &str, text: &str) -> Result<TranTypeCode, ApiError> {
    if text.contains('\0') {
        return Err(rejected(field, HAS_NUL));
    }
    // `from_trimmed`, not the COBOL `MOVE` of `move_from`: a code longer than
    // the X(02) field is rejected rather than cut to two characters.
    TranTypeCode::from_trimmed(text).map_err(|err| rejected(field, err))
}

/// Whether `text` is `-?[0-9]+(.[0-9]+)?`.
///
/// `Decimal`'s own parser also accepts `+1`, `.5`, `1.` and `1_000`; this
/// check keeps the accepted text to the pattern the `OpenAPI` document
/// publishes.
fn is_plain_decimal(text: &str) -> bool {
    let unsigned = text.strip_prefix('-').unwrap_or(text);
    let (integer, fraction) = match unsigned.split_once('.') {
        Some((integer, fraction)) => (integer, Some(fraction)),
        None => (unsigned, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    digits(integer) && fraction.is_none_or(digits)
}

/// The exact `Decimal` written as `text` in request field `field`: its sign,
/// its digits and its scale as written.
fn decimal(field: &str, text: &str) -> Result<Decimal, ApiError> {
    if !is_plain_decimal(text) {
        return Err(rejected(field, NOT_A_DECIMAL));
    }
    Decimal::from_str_exact(text).map_err(|err| rejected(field, err))
}

/// An amount bound of request field `field` as the `S9(09)V99` it is compared
/// with; `None` stays unbounded.
///
/// More than two fraction digits are rejected as written, even trailing zeros
/// (`1.230`), as the published pattern rejects them.
fn optional_amount(field: &str, text: Option<&str>) -> Result<Option<Money<9>>, ApiError> {
    text.map(|text| {
        let value = decimal(field, text)?;
        if value.scale() > AMOUNT_SCALE {
            return Err(rejected(
                field,
                format_args!("value has more than {AMOUNT_SCALE} decimal places"),
            ));
        }
        Money::<9>::from_decimal(value).map_err(|err| rejected(field, err))
    })
    .transpose()
}

/// The UTC instant written as an RFC 3339 date-time in request field `field`.
fn instant(field: &str, text: &str) -> Result<DateTime<Utc>, ApiError> {
    DateTime::parse_from_rfc3339(text)
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|err| rejected(field, err))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::entities::RewardRule;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages::{
        REWARD_ACTIVE_TO_BEFORE_FROM, REWARD_MIN_EXCEEDS_MAX, REWARD_RATE_NEGATIVE,
    };
    use carddemo_domain::numeric::{Money, RewardRate, RuleId, TranCategoryCode, TranTypeCode};
    use carddemo_domain::rewards::validate_rule;
    use chrono::{DateTime, TimeZone, Utc};
    use rust_decimal::Decimal;
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    use super::{RewardRuleField, RewardRuleRequest, RewardRuleResponse};
    use crate::dto::BmsField;
    use crate::error::ApiError;

    const FIELDS: [RewardRuleField; 3] = [
        RewardRuleField::RewardRate,
        RewardRuleField::MinTransactionAmount,
        RewardRuleField::ActiveTo,
    ];

    fn instant(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0)
            .single()
            .expect("a valid UTC instant")
    }

    fn decimal(text: &str) -> Decimal {
        Decimal::from_str_exact(text).expect("a decimal literal")
    }

    /// The retail seed rule of `batch seed` as the admin form submits it: type
    /// `01` Purchase, category `0001` Regular Sales Draft, rate 1.0000.
    fn retail_body() -> Value {
        json!({
            "transaction_type_cd": "01",
            "transaction_category_cd": "0001",
            "reward_rate": "1.0000",
            "active_from": "2022-01-01T00:00:00Z",
        })
    }

    /// `retail_body` with `field` set to `value`.
    fn body_with(field: &str, value: Value) -> Value {
        let mut body = retail_body();
        body[field] = value;
        body
    }

    fn request(body: Value) -> RewardRuleRequest {
        serde_json::from_value(body).expect("a well-formed request body")
    }

    fn rejection(body: Value) -> ApiError {
        request(body)
            .into_draft()
            .expect_err("the body does not convert")
    }

    #[test]
    fn retail_rule_converts_to_its_draft() {
        let draft = request(retail_body())
            .into_draft()
            .expect("the retail rule converts");
        assert_eq!(draft.transaction_type_cd, TranTypeCode::move_from("01"));
        assert_eq!(
            draft.transaction_category_cd,
            TranCategoryCode::new(1).expect("a 9(04) code")
        );
        assert_eq!(draft.reward_rate, decimal("1.0000"));
        assert_eq!(draft.reward_rate.scale(), 4);
        assert_eq!(draft.min_transaction_amount, None);
        assert_eq!(draft.max_transaction_amount, None);
        assert_eq!(draft.active_from, instant(2022, 1, 1, 0));
        assert_eq!(draft.active_to, None);
        assert!(validate_rule(draft).is_ok());
    }

    #[test]
    fn bounds_window_short_category_and_offsets_convert() {
        let draft = request(json!({
            "transaction_type_cd": "04",
            "transaction_category_cd": "3",
            "reward_rate": "3",
            "min_transaction_amount": "-0.5",
            "max_transaction_amount": "999999999.99",
            "active_from": "2022-01-01T02:00:00+02:00",
            "active_to": "2030-01-01T00:00:00.000Z",
        }))
        .into_draft()
        .expect("the travel rule converts");
        assert_eq!(draft.transaction_type_cd, TranTypeCode::move_from("04"));
        assert_eq!(
            draft.transaction_category_cd,
            TranCategoryCode::new(3).expect("a 9(04) code")
        );
        assert_eq!(draft.reward_rate, Decimal::from(3));
        assert_eq!(
            draft.min_transaction_amount,
            Some(Money::<9>::from_cents(-50).expect("an S9(09)V99 amount"))
        );
        assert_eq!(
            draft.max_transaction_amount,
            Some(Money::<9>::from_cents(99_999_999_999).expect("an S9(09)V99 amount"))
        );
        assert_eq!(draft.active_from, instant(2022, 1, 1, 0));
        assert_eq!(draft.active_to, Some(instant(2030, 1, 1, 0)));
    }

    #[test]
    fn explicit_nulls_are_unbounded_and_open_ended() {
        let mut body = retail_body();
        for field in [
            "min_transaction_amount",
            "max_transaction_amount",
            "active_to",
        ] {
            body[field] = Value::Null;
        }
        let draft = request(body).into_draft().expect("nulls convert");
        assert_eq!(draft.min_transaction_amount, None);
        assert_eq!(draft.max_transaction_amount, None);
        assert_eq!(draft.active_to, None);
    }

    #[test]
    fn negative_rate_converts_and_the_domain_rejects_it() {
        let draft = request(body_with("reward_rate", json!("-1")))
            .into_draft()
            .expect("a negative rate converts");
        assert_eq!(draft.reward_rate, Decimal::from(-1));

        let error = ApiError::from(validate_rule(draft).expect_err("a negative rate"));
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.body().code, ErrorKind::Invalid.code());
        assert_eq!(error.body().message, REWARD_RATE_NEGATIVE);
        let fields: Vec<&str> = error
            .body()
            .fields
            .iter()
            .map(|entry| entry.field.as_str())
            .collect();
        assert_eq!(fields, [RewardRuleField::RewardRate.as_str()]);
    }

    #[test]
    fn field_enum_names_the_fields_validate_rule_reports_in_rank_order() {
        let draft = request(json!({
            "transaction_type_cd": "01",
            "transaction_category_cd": "1",
            "reward_rate": "-0.0001",
            "min_transaction_amount": "100.00",
            "max_transaction_amount": "50.00",
            "active_from": "2022-07-01T00:00:00Z",
            "active_to": "2022-06-30T23:59:59Z",
        }))
        .into_draft()
        .expect("every value converts");
        let error = validate_rule(draft).expect_err("every check fails");
        let reported: Vec<(&str, &str)> = error
            .fields
            .iter()
            .map(|entry| (entry.field, entry.message.as_str()))
            .collect();
        let mut ranked = FIELDS;
        ranked.sort_by_key(|field| BmsField::rank(*field));
        let messages = [
            REWARD_RATE_NEGATIVE,
            REWARD_MIN_EXCEEDS_MAX,
            REWARD_ACTIVE_TO_BEFORE_FROM,
        ];
        let expected: Vec<(&str, &str)> = ranked
            .iter()
            .map(|field| field.as_str())
            .zip(messages)
            .collect();
        assert_eq!(reported, expected);
    }

    #[test]
    fn field_names_agree_across_serde_inherent_and_trait() {
        for field in FIELDS {
            let wire = serde_json::to_value(field).expect("serializes");
            assert_eq!(wire, json!(field.as_str()), "{field:?}");
            assert_eq!(BmsField::as_str(field), field.as_str(), "{field:?}");
            let back: RewardRuleField = serde_json::from_value(wire).expect("deserializes");
            assert_eq!(back, field);
        }
        assert!(serde_json::from_value::<RewardRuleField>(json!("RewardRate")).is_err());
    }

    #[test]
    fn unconvertible_text_is_a_400_rejection_naming_the_field() {
        let cases = [
            ("transaction_type_cd", json!("012")),
            ("transaction_type_cd", json!("\u{100}")),
            ("transaction_type_cd", json!("0\u{0}")),
            ("transaction_type_cd", json!("\u{0}")),
            ("transaction_type_cd", json!("\u{0}\u{0}")),
            ("transaction_category_cd", json!("")),
            ("transaction_category_cd", json!("12345")),
            ("transaction_category_cd", json!("-1")),
            ("transaction_category_cd", json!("1a")),
            ("reward_rate", json!("abc")),
            ("reward_rate", json!("")),
            ("reward_rate", json!("+1")),
            ("reward_rate", json!(".5")),
            ("reward_rate", json!("1.")),
            ("reward_rate", json!("1_000")),
            ("reward_rate", json!("1e3")),
            ("reward_rate", json!("--1")),
            ("reward_rate", json!("1".repeat(40))),
            ("min_transaction_amount", json!("1.234")),
            ("min_transaction_amount", json!("1.230")),
            ("min_transaction_amount", json!("ten")),
            ("max_transaction_amount", json!("1000000000.00")),
            ("max_transaction_amount", json!("-1000000000")),
            ("active_from", json!("2022-01-01")),
            ("active_from", json!("2022-01-01 00:00:00")),
            ("active_to", json!("2022-13-01T00:00:00Z")),
            ("active_to", json!("")),
        ];
        for (field, value) in cases {
            let error = rejection(body_with(field, value.clone()));
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{field} {value}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{field}");
            assert!(
                error.body().message.starts_with(&format!("{field}: ")),
                "{field} {value}: {}",
                error.body().message
            );
            assert!(error.body().fields.is_empty(), "{field} {value}");
        }
    }

    #[test]
    fn rejection_reasons_say_what_is_wrong() {
        let cases = [
            ("reward_rate", json!("abc"), "value is not a decimal number"),
            (
                "min_transaction_amount",
                json!("1.234"),
                "value has more than 2 decimal places",
            ),
            (
                "max_transaction_amount",
                json!("1000000000.00"),
                "value is outside the range of the field",
            ),
            (
                "transaction_type_cd",
                json!("012"),
                "expected at most 2 characters, got 3",
            ),
            (
                "transaction_type_cd",
                json!("0\u{0}"),
                "value has a NUL character (U+0000)",
            ),
            (
                "transaction_category_cd",
                json!("x"),
                "value is not numeric",
            ),
        ];
        for (field, value, reason) in cases {
            assert_eq!(
                rejection(body_with(field, value)).body().message,
                format!("{field}: {reason}")
            );
        }
    }

    #[test]
    fn type_codes_of_iso_8859_1_characters_but_nul_convert() {
        for code in ["01", "0", "\u{e9}", "\u{1}", "\u{ff}\u{ff}"] {
            let draft = request(body_with("transaction_type_cd", json!(code)))
                .into_draft()
                .unwrap_or_else(|error| panic!("{code:?}: {}", error.body().message));
            assert_eq!(
                draft.transaction_type_cd,
                TranTypeCode::from_trimmed(code).expect("an X(02) code"),
                "{code:?}"
            );
            assert_eq!(draft.transaction_type_cd.trimmed(), code, "{code:?}");
        }
    }

    #[test]
    fn missing_required_fields_do_not_deserialize() {
        for field in [
            "transaction_type_cd",
            "transaction_category_cd",
            "reward_rate",
            "active_from",
        ] {
            let mut body = retail_body();
            body.as_object_mut().expect("an object").remove(field);
            let error = serde_json::from_value::<RewardRuleRequest>(body)
                .expect_err("a required field is missing");
            assert!(error.to_string().contains(field), "{error}");
        }
        assert!(
            serde_json::from_value::<RewardRuleRequest>(body_with("reward_rate", json!(1)))
                .is_err(),
            "a JSON number is not a decimal string"
        );
    }

    fn retail_rule() -> RewardRule {
        RewardRule {
            rule_id: RuleId::from(1),
            transaction_type_cd: TranTypeCode::move_from("01"),
            transaction_category_cd: TranCategoryCode::new(1).expect("a 9(04) code"),
            reward_rate: RewardRate::from_decimal(Decimal::ONE).expect("a NUMERIC(9,4) rate"),
            min_transaction_amount: None,
            max_transaction_amount: None,
            active_from: instant(2022, 1, 1, 0),
            active_to: None,
        }
    }

    #[test]
    fn response_serializes_every_column_as_text() {
        let json =
            serde_json::to_value(RewardRuleResponse::from(&retail_rule())).expect("serializes");
        assert_eq!(
            json,
            json!({
                "rule_id": "1",
                "transaction_type_cd": "01",
                "transaction_category_cd": "0001",
                "reward_rate": "1.0000",
                "min_transaction_amount": null,
                "max_transaction_amount": null,
                "active_from": "2022-01-01T00:00:00Z",
                "active_to": null,
            })
        );
    }

    #[test]
    fn response_shows_bounds_at_scale_two_and_a_soft_delete_instant() {
        let rule = RewardRule {
            rule_id: RuleId::from(9_007_199_254_740_993),
            transaction_type_cd: TranTypeCode::move_from("4"),
            transaction_category_cd: TranCategoryCode::new(3).expect("a 9(04) code"),
            reward_rate: RewardRate::from_decimal(decimal("1.5")).expect("a NUMERIC(9,4) rate"),
            min_transaction_amount: Some(Money::from_cents(1_000).expect("an amount")),
            max_transaction_amount: Some(Money::from_cents(-5).expect("an amount")),
            active_to: Some(instant(2022, 7, 1, 12)),
            ..retail_rule()
        };
        let json = serde_json::to_value(RewardRuleResponse::from(&rule)).expect("serializes");
        assert_eq!(json["rule_id"], json!("9007199254740993"));
        assert_eq!(json["transaction_type_cd"], json!("4"));
        assert_eq!(json["transaction_category_cd"], json!("0003"));
        assert_eq!(json["reward_rate"], json!("1.5000"));
        assert_eq!(json["min_transaction_amount"], json!("10.00"));
        assert_eq!(json["max_transaction_amount"], json!("-0.05"));
        assert_eq!(json["active_to"], json!("2022-07-01T12:00:00Z"));
    }

    fn schema_json<T: PartialSchema>() -> Value {
        serde_json::to_value(T::schema()).expect("the schema serializes")
    }

    #[test]
    fn request_schema_publishes_required_fields_and_patterns() {
        let schema = schema_json::<RewardRuleRequest>();
        assert_eq!(
            schema["required"],
            json!([
                "transaction_type_cd",
                "transaction_category_cd",
                "reward_rate",
                "active_from"
            ])
        );
        let property = |name: &str| schema["properties"][name].clone();
        assert_eq!(property("transaction_type_cd")["maxLength"], json!(2));
        assert_eq!(
            property("transaction_type_cd")["pattern"],
            json!(r"^[\x01-\xFF]*$")
        );
        assert_eq!(
            property("transaction_category_cd")["pattern"],
            json!("^[0-9]{1,4}$")
        );
        assert_eq!(
            property("reward_rate")["pattern"],
            json!(r"^-?[0-9]+(\.[0-9]+)?$")
        );
        for bound in ["min_transaction_amount", "max_transaction_amount"] {
            assert_eq!(
                property(bound)["pattern"],
                json!(r"^-?[0-9]+(\.[0-9]{1,2})?$"),
                "{bound}"
            );
        }
        for name in ["active_from", "active_to"] {
            assert_eq!(property(name)["format"], json!("date-time"), "{name}");
        }
    }

    #[test]
    fn response_and_field_schemas_are_strings_throughout() {
        let schema = schema_json::<RewardRuleResponse>();
        let required = schema["required"].as_array().expect("a required list");
        assert_eq!(required.len(), 8, "{schema}");
        for (name, property) in schema["properties"].as_object().expect("properties") {
            let kind = &property["type"];
            assert!(
                *kind == json!("string") || *kind == json!(["string", "null"]),
                "{name}: {property}"
            );
        }
        let field = schema_json::<RewardRuleField>();
        assert_eq!(
            field["enum"],
            json!(["reward_rate", "min_transaction_amount", "active_to"])
        );
    }
}
