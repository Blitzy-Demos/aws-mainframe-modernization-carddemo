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

use std::ops::RangeInclusive;

use carddemo_domain::clock::parse_instant;
use carddemo_domain::numeric::{Money, TranCategoryCode, TranTypeCode};
use carddemo_domain::rules::{RewardRule, RuleDraft};
use chrono::{DateTime, Datelike, Utc};
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

/// The years an RFC 3339 date-time can write: `date-fullyear` is exactly four
/// digits.
const RFC_3339_YEARS: RangeInclusive<i32> = 0..=9999;

/// The rejection reason of an instant whose UTC year is outside
/// [`RFC_3339_YEARS`], which a response could not write back as RFC 3339.
const OUTSIDE_RFC_3339_YEARS: &str = "value is outside 0000-01-01T00:00:00Z to \
    9999-12-31T23:59:59.999999Z once converted to UTC, which an RFC 3339 date-time \
    cannot write";

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
    /// The inclusive lower bound of a qualifying amount, a decimal string from
    /// -999999999.99 to 999999999.99 with at most 2 fraction digits
    /// (`S9(09)V99`); leading zeros are allowed. Absent or `null` is
    /// unbounded.
    // The pattern is exactly the text `optional_amount` accepts: at most 9
    // integer digits after any leading zeros, a magnitude below 10^9.
    #[serde(default)]
    #[schema(pattern = r"^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$", example = "10.00")]
    pub min_transaction_amount: Option<String>,
    /// The inclusive upper bound of a qualifying amount, a decimal string from
    /// -999999999.99 to 999999999.99 with at most 2 fraction digits
    /// (`S9(09)V99`); leading zeros are allowed. Absent or `null` is
    /// unbounded.
    // The same pattern as `min_transaction_amount`.
    #[serde(default)]
    #[schema(pattern = r"^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$", example = "500.00")]
    pub max_transaction_amount: Option<String>,
    /// The first instant the rule is active, inclusive, as an RFC 3339
    /// date-time of at most microsecond precision, converted to UTC: a
    /// fraction digit after the 6th must be `0`, and second `60` (a leap
    /// second) is refused, because the stored timestamp keeps whole
    /// microseconds and no leap second. The day must exist in its month, and
    /// the instant converted to UTC must lie in years 0000 to 9999, the
    /// four-digit years an RFC 3339 date-time writes, so that the rule is
    /// returned in a form this endpoint accepts back.
    // The pattern is `carddemo_domain::clock::INSTANT_PATTERN`, the shape
    // `instant` accepts; the attribute takes only a literal.
    #[schema(
        format = DateTime,
        pattern = r"^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])[Tt ]([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]{1,6}0*)?([Zz]|[+\-\u2212]([01][0-9]|2[0-3]):[0-5][0-9])$",
        example = "2022-01-01T00:00:00Z"
    )]
    pub active_from: String,
    /// The end of the active window, exclusive, as an RFC 3339 date-time of
    /// at most microsecond precision, converted to UTC, which must lie in
    /// years 0000 to 9999, held to the same constraints as `active_from`.
    /// Absent or `null` leaves the rule open-ended.
    #[serde(default)]
    #[schema(
        format = DateTime,
        pattern = r"^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])[Tt ]([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]{1,6}0*)?([Zz]|[+\-\u2212]([01][0-9]|2[0-3]):[0-5][0-9])$",
        example = "2030-01-01T00:00:00Z"
    )]
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
    ///   `-?[0-9]+(.[0-9]{1,2})?`, or a magnitude of 10^9 or more; together,
    ///   the text the published `^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$` rejects.
    /// - `active_from`, `active_to`: not an RFC 3339 date-time; a fraction
    ///   digit after the 6th that is not `0`, finer than a microsecond;
    ///   second `60`, a leap second; or, checked after those, an instant
    ///   whose UTC conversion is outside years 0000 to 9999, which the
    ///   response could not write as RFC 3339. None of them is narrowed to
    ///   fit.
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

/// Where the processing clock's instant of an answer lies against a rule's
/// active window, from `active_from` (inclusive) to `active_to` (exclusive)
/// (AAP 0.3.5).
///
/// The instant is the server's processing clock (`CARDDEMO_CLOCK_OVERRIDE`
/// pins it, AAP 0.6.4), the one the reward engine matches transactions
/// against, so a client never has to judge a rule by a clock of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RewardRuleStatus {
    /// `scheduled`: the rule has not started, because `active_from` is later
    /// than the instant, and its window has not been closed by then.
    Scheduled,
    /// `active`: the window holds the instant, so the rule matches a
    /// transaction processed then.
    Active,
    /// `ended`: `active_to` is at or before the instant, as a soft delete
    /// leaves it, so the rule matches no transaction from then on.
    Ended,
}

impl RewardRuleStatus {
    /// The status of `rule` at instant `now`.
    ///
    /// Active is exactly [`RewardRule::is_active_at`], the one definition of
    /// the window. Otherwise a rule whose `active_to` is at or before `now`
    /// has ended, which covers a rule soft-deleted before it started (its
    /// `active_to` then precedes `active_from`); any other rule starts later.
    #[must_use]
    pub fn of(rule: &RewardRule, now: DateTime<Utc>) -> Self {
        if rule.is_active_at(now) {
            Self::Active
        } else if rule.active_to.is_some_and(|end| end <= now) {
            Self::Ended
        } else {
            Self::Scheduled
        }
    }
}

/// One stored reward rule: every column of a `reward_rules` row, plus its
/// `status` at the processing clock's instant of the answer, as the list,
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
    /// The first instant the rule is active, inclusive, in RFC 3339 UTC, in
    /// years 0000 to 9999 as the request requires.
    pub active_from: DateTime<Utc>,
    /// The end of the active window, exclusive, in RFC 3339 UTC, in years 0000
    /// to 9999 as the request requires; `null` for an open-ended rule.
    #[schema(required = true)]
    pub active_to: Option<DateTime<Utc>>,
    /// The rule's state at the processing clock's instant of this answer:
    /// `scheduled`, `active` or `ended`. The instant is the server's, never
    /// the client's.
    pub status: RewardRuleStatus,
}

impl RewardRuleResponse {
    /// The answer for `rule` at `now`, the processing clock's instant of the
    /// request (AAP 0.6.4), which decides its `status`.
    ///
    /// A handler reads the clock once per request and builds every rule of
    /// its answer at that instant, so the rules of one list are judged alike.
    #[must_use]
    pub fn at(rule: &RewardRule, now: DateTime<Utc>) -> Self {
        Self {
            rule_id: id_string(&rule.rule_id),
            transaction_type_cd: id_string(&rule.transaction_type_cd),
            transaction_category_cd: id_string(&rule.transaction_category_cd),
            reward_rate: rule.reward_rate.to_decimal(),
            min_transaction_amount: rule.min_transaction_amount.map(money),
            max_transaction_amount: rule.max_transaction_amount.map(money),
            active_from: rule.active_from,
            active_to: rule.active_to,
            status: RewardRuleStatus::of(rule, now),
        }
    }
}

/// What a `DELETE /admin/reward-rules/{id}` of an existing rule did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RewardRuleDeleteOutcome {
    /// `deleted`: the rule's `active_to` was absent or later than the delete
    /// instant and is now that instant, so the rule has ended.
    Deleted,
    /// `unchanged`: the rule had already ended at the delete instant, so
    /// nothing was written.
    Unchanged,
}

impl RewardRuleDeleteOutcome {
    /// The wire value, which is also the `outcome` the handler span records.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Unchanged => "unchanged",
        }
    }
}

/// The 200 answer of `DELETE /admin/reward-rules/{id}`: the rule after the
/// soft delete, under the same top-level keys as `RewardRuleResponse`, with
/// `outcome` beside them, so a client reports what the server did rather than
/// guess it from the rule it showed before.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RewardRuleDeleteResponse {
    /// The rule as stored after the delete, its `status` at the delete
    /// instant: always `ended`.
    #[serde(flatten)]
    pub rule: RewardRuleResponse,
    /// Whether the delete ended the rule or found it already ended.
    pub outcome: RewardRuleDeleteOutcome,
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
/// (`1.230`), as the published pattern rejects them. So is a magnitude of
/// 10^9 or more (`1000000000.00`), the published pattern's limit of 9 integer
/// digits after any leading zeros (`0000000001.00` is accepted).
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

/// The UTC instant written as an RFC 3339 date-time in request field `field`,
/// through [`parse_instant`]: an instant finer than a microsecond or on a leap
/// second, which `reward_rules` would not store as written, is rejected rather
/// than narrowed.
///
/// So is an instant whose UTC year, once the offset is applied, is outside
/// years 0000 to 9999, such as `9999-12-31T23:59:59-05:00` or
/// `0000-01-01T00:00:00+01:00`. Its text has the four-digit year the
/// published pattern allows, but the response would write the stored instant
/// as `+10000-01-01T04:59:59Z` or `-0001-12-31T23:00:00Z`, which is not
/// RFC 3339 and which this function would refuse when the rule is sent back.
/// The range is checked after [`parse_instant`]'s own refusals.
fn instant(field: &str, text: &str) -> Result<DateTime<Utc>, ApiError> {
    let instant = parse_instant(text).map_err(|err| rejected(field, err))?;
    if !RFC_3339_YEARS.contains(&instant.year()) {
        return Err(rejected(field, OUTSIDE_RFC_3339_YEARS));
    }
    Ok(instant)
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::clock::INSTANT_PATTERN;
    use carddemo_domain::entities::RewardRule;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages::{
        REWARD_ACTIVE_TO_BEFORE_FROM, REWARD_MIN_EXCEEDS_MAX, REWARD_RATE_NEGATIVE,
    };
    use carddemo_domain::numeric::{Money, RewardRate, RuleId, TranCategoryCode, TranTypeCode};
    use carddemo_domain::rewards::validate_rule;
    use chrono::{DateTime, TimeDelta, TimeZone, Utc};
    use rust_decimal::Decimal;
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    use super::{
        RewardRuleDeleteOutcome, RewardRuleDeleteResponse, RewardRuleField, RewardRuleRequest,
        RewardRuleResponse, RewardRuleStatus,
    };
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

    /// The published amount pattern `^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$` holds the
    /// range `optional_amount` enforces: up to 9 integer digits after any
    /// leading zeros convert, at both bounds of `S9(09)V99`; a tenth
    /// significant integer digit is out of range, however many zeros lead.
    #[test]
    fn amount_bounds_convert_up_to_nine_integer_digits_after_leading_zeros() {
        let leading_zeros = format!("{}1.00", "0".repeat(40));
        let accepted = [
            ("999999999.99", 99_999_999_999),
            ("-999999999.99", -99_999_999_999),
            ("0000000001.00", 100),
            ("000000000999999999.99", 99_999_999_999),
            (leading_zeros.as_str(), 100),
            ("123456789.5", 12_345_678_950),
            ("0", 0),
            ("-0.00", 0),
        ];
        for field in ["min_transaction_amount", "max_transaction_amount"] {
            for (text, cents) in accepted {
                let draft = request(body_with(field, json!(text)))
                    .into_draft()
                    .unwrap_or_else(|error| panic!("{field} {text}: {}", error.body().message));
                let bound = if field == "min_transaction_amount" {
                    draft.min_transaction_amount
                } else {
                    draft.max_transaction_amount
                };
                assert_eq!(
                    bound,
                    Some(Money::<9>::from_cents(cents).expect("an S9(09)V99 amount")),
                    "{field} {text}"
                );
            }
            for text in [
                "1000000000.00",
                "-1000000000",
                "01000000000",
                "0000000001000000000.5",
                "-9999999999.99",
            ] {
                assert_eq!(
                    rejection(body_with(field, json!(text))).body().message,
                    format!("{field}: value is outside the range of the field"),
                    "{text}"
                );
            }
        }
    }

    /// 2022-07-01T12:00:00Z plus `micros` microseconds.
    fn noon_plus(micros: i64) -> DateTime<Utc> {
        instant(2022, 7, 1, 12) + TimeDelta::microseconds(micros)
    }

    #[test]
    fn whole_microsecond_instants_convert_to_utc_as_written() {
        let cases = [
            ("2022-07-01T12:00:00Z", noon_plus(0)),
            ("2022-07-01T12:00:00.000001Z", noon_plus(1)),
            ("2022-07-01T12:00:00.000001000Z", noon_plus(1)),
            ("2022-07-01T12:00:00.000000000Z", noon_plus(0)),
            ("2022-07-01T14:00:00.000002+02:00", noon_plus(2)),
            ("2022-07-01T07:00:00\u{2212}05:00", noon_plus(0)),
            ("2022-07-01 12:00:00.5z", noon_plus(500_000)),
        ];
        for (text, expected) in cases {
            let draft = request(body_with("active_from", json!(text)))
                .into_draft()
                .unwrap_or_else(|error| panic!("active_from {text}: {}", error.body().message));
            assert_eq!(draft.active_from, expected, "active_from {text}");
            let draft = request(body_with("active_to", json!(text)))
                .into_draft()
                .unwrap_or_else(|error| panic!("active_to {text}: {}", error.body().message));
            assert_eq!(draft.active_to, Some(expected), "active_to {text}");
        }
    }

    #[test]
    fn instants_finer_than_a_microsecond_or_on_a_leap_second_are_rejected() {
        let finer = "value is finer than a microsecond: a fraction digit after the 6th is not 0";
        let leap = "value is a leap second (second 60), which a timestamp cannot hold";
        let cases = [
            ("2022-07-01T12:00:00.000000900Z", finer),
            ("2022-07-01T12:00:00.0000000001Z", finer),
            ("2016-12-31T23:59:60Z", leap),
        ];
        for field in ["active_from", "active_to"] {
            for (text, reason) in cases {
                let error = rejection(body_with(field, json!(text)));
                assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{field} {text}");
                assert_eq!(
                    error.body().code,
                    ErrorKind::Invalid.code(),
                    "{field} {text}"
                );
                assert_eq!(error.body().message, format!("{field}: {reason}"));
                assert!(error.body().fields.is_empty(), "{field} {text}");
            }
        }
        assert!(
            rejection(body_with("active_from", json!("2022-01-01")))
                .body()
                .message
                .starts_with("active_from: value is not an RFC 3339 date-time: "),
            "a date alone is not an instant"
        );
    }

    #[test]
    fn instants_whose_utc_year_is_outside_0000_to_9999_are_rejected() {
        let outside = "value is outside 0000-01-01T00:00:00Z to 9999-12-31T23:59:59.999999Z \
            once converted to UTC, which an RFC 3339 date-time cannot write";
        let finer = "value is finer than a microsecond: a fraction digit after the 6th is not 0";
        let leap = "value is a leap second (second 60), which a timestamp cannot hold";
        let cases = [
            ("9999-12-31T23:59:59-05:00", outside),
            ("9999-12-31T23:59:59.999999-00:01", outside),
            ("0000-01-01T00:00:00+01:00", outside),
            ("0000-01-01T00:59:59.999999+01:00", outside),
            // `parse_instant`'s own refusals are answered before the range.
            ("9999-12-31T23:59:59.0000001-05:00", finer),
            ("9999-12-31T23:59:60-05:00", leap),
        ];
        for field in ["active_from", "active_to"] {
            for (text, reason) in cases {
                let error = rejection(body_with(field, json!(text)));
                assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{field} {text}");
                assert_eq!(
                    error.body().code,
                    ErrorKind::Invalid.code(),
                    "{field} {text}"
                );
                assert_eq!(error.body().message, format!("{field}: {reason}"));
                assert!(error.body().fields.is_empty(), "{field} {text}");
            }
        }
    }

    #[test]
    fn instants_at_the_utc_year_bounds_convert_and_echo_back() {
        let last = Utc
            .with_ymd_and_hms(9999, 12, 31, 23, 59, 59)
            .single()
            .expect("a valid UTC instant")
            + TimeDelta::microseconds(999_999);
        let first = instant(0, 1, 1, 0);
        let cases = [
            (
                "9999-12-31T23:59:59.999999Z",
                last,
                "9999-12-31T23:59:59.999999Z",
            ),
            (
                "9999-12-31T18:59:59.999999-05:00",
                last,
                "9999-12-31T23:59:59.999999Z",
            ),
            ("0000-01-01T00:00:00Z", first, "0000-01-01T00:00:00Z"),
            ("0000-01-01T01:00:00+01:00", first, "0000-01-01T00:00:00Z"),
        ];
        for (text, expected, written) in cases {
            let mut body = body_with("active_from", json!(text));
            body["active_to"] = json!(text);
            let draft = request(body)
                .into_draft()
                .unwrap_or_else(|error| panic!("{text}: {}", error.body().message));
            assert_eq!(draft.active_from, expected, "active_from {text}");
            assert_eq!(draft.active_to, Some(expected), "active_to {text}");

            // The GET → PUT echo: the rule as the response writes it is
            // accepted back as the same instants.
            let rule = RewardRule {
                active_from: draft.active_from,
                active_to: draft.active_to,
                ..retail_rule()
            };
            let response = serde_json::to_value(RewardRuleResponse::at(&rule, noon_plus(0)))
                .expect("serializes");
            let mut echo = retail_body();
            for field in ["active_from", "active_to"] {
                assert_eq!(response[field], json!(written), "{field} {text}");
                echo[field] = response[field].clone();
            }
            let echoed = request(echo)
                .into_draft()
                .unwrap_or_else(|error| panic!("echo of {text}: {}", error.body().message));
            assert_eq!(echoed.active_from, expected, "echoed active_from {text}");
            assert_eq!(echoed.active_to, Some(expected), "echoed active_to {text}");
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
    fn response_serializes_every_column_as_text_and_the_status() {
        let json = serde_json::to_value(RewardRuleResponse::at(&retail_rule(), noon_plus(0)))
            .expect("serializes");
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
                "status": "active",
            })
        );
    }

    /// `retail_rule` with the window from `active_from` to `active_to`.
    fn window(active_from: DateTime<Utc>, active_to: Option<DateTime<Utc>>) -> RewardRule {
        RewardRule {
            active_from,
            active_to,
            ..retail_rule()
        }
    }

    #[test]
    fn status_places_the_instant_against_the_window() {
        let start = instant(2022, 1, 1, 0);
        let end = instant(2022, 7, 2, 0);
        let micro = TimeDelta::microseconds(1);
        // (case, rule, instant, status)
        let cases = [
            (
                "before the start",
                window(start, Some(end)),
                start - micro,
                RewardRuleStatus::Scheduled,
            ),
            (
                "at the start",
                window(start, Some(end)),
                start,
                RewardRuleStatus::Active,
            ),
            (
                "inside the window",
                window(start, Some(end)),
                noon_plus(0),
                RewardRuleStatus::Active,
            ),
            (
                "just before the end",
                window(start, Some(end)),
                end - micro,
                RewardRuleStatus::Active,
            ),
            (
                "at the end",
                window(start, Some(end)),
                end,
                RewardRuleStatus::Ended,
            ),
            (
                "after the end",
                window(start, Some(end)),
                end + micro,
                RewardRuleStatus::Ended,
            ),
            (
                "open-ended, before the start",
                window(start, None),
                start - micro,
                RewardRuleStatus::Scheduled,
            ),
            (
                "open-ended, long after the start",
                window(start, None),
                instant(9999, 12, 31, 23),
                RewardRuleStatus::Active,
            ),
            (
                "empty window, before it",
                window(end, Some(end)),
                end - micro,
                RewardRuleStatus::Scheduled,
            ),
            (
                "empty window, at it",
                window(end, Some(end)),
                end,
                RewardRuleStatus::Ended,
            ),
            (
                "deleted before its start, its end preceding its start",
                window(instant(2030, 1, 1, 0), Some(noon_plus(0))),
                noon_plus(0),
                RewardRuleStatus::Ended,
            ),
        ];
        for (case, rule, now, status) in cases {
            assert_eq!(RewardRuleStatus::of(&rule, now), status, "{case}");
            assert_eq!(
                rule.is_active_at(now),
                status == RewardRuleStatus::Active,
                "{case}: active is exactly the domain window"
            );
            let json =
                serde_json::to_value(RewardRuleResponse::at(&rule, now)).expect("serializes");
            assert_eq!(
                json["status"],
                serde_json::to_value(status).expect("serializes")
            );
        }
    }

    #[test]
    fn status_and_outcome_names_are_lowercase_words() {
        for (status, wire) in [
            (RewardRuleStatus::Scheduled, "scheduled"),
            (RewardRuleStatus::Active, "active"),
            (RewardRuleStatus::Ended, "ended"),
        ] {
            assert_eq!(
                serde_json::to_value(status).expect("serializes"),
                json!(wire)
            );
        }
        for (outcome, wire) in [
            (RewardRuleDeleteOutcome::Deleted, "deleted"),
            (RewardRuleDeleteOutcome::Unchanged, "unchanged"),
        ] {
            assert_eq!(outcome.as_str(), wire);
            assert_eq!(
                serde_json::to_value(outcome).expect("serializes"),
                json!(wire)
            );
        }
    }

    #[test]
    fn delete_response_is_the_rule_keys_plus_the_outcome() {
        let rule = window(instant(2022, 1, 1, 0), Some(noon_plus(0)));
        let answer = RewardRuleDeleteResponse {
            rule: RewardRuleResponse::at(&rule, noon_plus(0)),
            outcome: RewardRuleDeleteOutcome::Deleted,
        };
        let mut expected =
            serde_json::to_value(RewardRuleResponse::at(&rule, noon_plus(0))).expect("serializes");
        expected["outcome"] = json!("deleted");
        assert_eq!(serde_json::to_value(&answer).expect("serializes"), expected);
        assert_eq!(expected["status"], json!("ended"));
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
        let json =
            serde_json::to_value(RewardRuleResponse::at(&rule, noon_plus(0))).expect("serializes");
        assert_eq!(json["rule_id"], json!("9007199254740993"));
        assert_eq!(json["transaction_type_cd"], json!("4"));
        assert_eq!(json["transaction_category_cd"], json!("0003"));
        assert_eq!(json["reward_rate"], json!("1.5000"));
        assert_eq!(json["min_transaction_amount"], json!("10.00"));
        assert_eq!(json["max_transaction_amount"], json!("-0.05"));
        assert_eq!(json["active_to"], json!("2022-07-01T12:00:00Z"));
        assert_eq!(json["status"], json!("ended"));
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
                json!(r"^-?0*[0-9]{1,9}(\.[0-9]{1,2})?$"),
                "{bound}"
            );
        }
        for name in ["active_from", "active_to"] {
            assert_eq!(property(name)["format"], json!("date-time"), "{name}");
            assert_eq!(property(name)["pattern"], json!(INSTANT_PATTERN), "{name}");
        }
    }

    #[test]
    fn response_and_field_schemas_are_strings_throughout() {
        let schema = schema_json::<RewardRuleResponse>();
        let required = schema["required"].as_array().expect("a required list");
        assert_eq!(required.len(), 9, "{schema}");
        assert!(required.contains(&json!("status")), "{schema}");
        for (name, property) in schema["properties"].as_object().expect("properties") {
            if name == "status" {
                assert_eq!(
                    property["$ref"],
                    json!("#/components/schemas/RewardRuleStatus"),
                    "{property}"
                );
                continue;
            }
            let kind = &property["type"];
            assert!(
                *kind == json!("string") || *kind == json!(["string", "null"]),
                "{name}: {property}"
            );
        }
        for (schema, values) in [
            (
                schema_json::<RewardRuleStatus>(),
                json!(["scheduled", "active", "ended"]),
            ),
            (
                schema_json::<RewardRuleDeleteOutcome>(),
                json!(["deleted", "unchanged"]),
            ),
        ] {
            assert_eq!(schema["type"], json!("string"), "{schema}");
            assert_eq!(schema["enum"], values, "{schema}");
        }
        let field = schema_json::<RewardRuleField>();
        assert_eq!(
            field["enum"],
            json!(["reward_rate", "min_transaction_amount", "active_to"])
        );
    }
}
