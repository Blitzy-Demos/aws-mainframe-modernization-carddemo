//! The wire types of the reward read endpoints (request Rewards system;
//! AAP 0.3.5): `GET /rewards/balance/{account_id}` and
//! `GET /rewards/history/{account_id}`.
//!
//! Rewards have no COBOL oracle, so no audit row or paragraph applies; the
//! field names are the columns of `reward_balances` and `reward_accruals` in
//! `migrations/0005_rewards.sql`, as [`RewardBalance`] and [`RewardAccrual`]
//! name them.
//!
//! # Points and identities are strings
//!
//! Reward points are `BIGINT` values up to `i64::MAX`, and a JSON number above
//! 2^53 − 1 loses precision in a browser: `9007199254740993` would arrive as
//! `9007199254740992`. Every points field is therefore the decimal digit
//! string of [`RewardPoints`] (`^[0-9]+$`, no sign, no separators), never a
//! JSON number and never a floating-point value (AAP 0.5.3.1, 0.10.3). The row
//! identities `accrual_id`, `transaction_id` and `rule_id` are decimal strings
//! for the same reason, and `account_id` is the 11-digit zero-padded
//! `ACCT-ID PIC 9(11)` [app/cpy/CVACT01Y.cpy:L5] as every other endpoint
//! sends it.

use std::fmt;

use carddemo_domain::entities::{RewardAccrual, RewardBalance};
use carddemo_domain::numeric::{AccrualId, RewardPoints};
use chrono::{DateTime, Utc};
use serde::de::{self, Error as _, Unexpected};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::id_string;
use crate::error::ApiError;
use crate::pagination::PageSize;

/// The wire text of reward points: the plain decimal integer.
fn points_text(points: RewardPoints) -> String {
    points.to_string()
}

/// The reward balance of one account: the body of
/// `GET /rewards/balance/{account_id}` (request Rewards system; AAP 0.3.5).
///
/// An existing account without accruals answers the zero balance, all three
/// totals `"0"`. `total_points_redeemed` is always `"0"`, because redemption is
/// out of scope (AAP 0.2.5); it is sent so that
/// `available_points = total_points_earned − total_points_redeemed`, the
/// `reward_balances` table `CHECK`, can be read off the body.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RewardBalanceResponse {
    /// The account, as its 11 zero-padded `ACCT-ID` digits.
    #[schema(pattern = "^[0-9]{11}$", example = "00000000001")]
    pub account_id: String,
    /// The sum of the account's accruals, as a decimal integer string.
    #[schema(value_type = String, pattern = "^[0-9]+$", example = "9007199254740993")]
    pub total_points_earned: String,
    /// The points spent, as a decimal integer string. Always `"0"`: redemption
    /// is out of scope.
    #[schema(value_type = String, pattern = "^[0-9]+$", example = "0")]
    pub total_points_redeemed: String,
    /// The points the account holds, `total_points_earned` minus
    /// `total_points_redeemed`, as a decimal integer string.
    #[schema(value_type = String, pattern = "^[0-9]+$", example = "9007199254740993")]
    pub available_points: String,
}

impl From<&RewardBalance> for RewardBalanceResponse {
    fn from(balance: &RewardBalance) -> Self {
        Self {
            account_id: id_string(&balance.account_id),
            total_points_earned: points_text(balance.total_points_earned),
            total_points_redeemed: points_text(balance.total_points_redeemed),
            available_points: points_text(balance.available_points),
        }
    }
}

/// One entry of an account's reward history: the points one qualifying
/// transaction earned, an item of `GET /rewards/history/{account_id}`
/// (request Rewards system; AAP 0.3.5).
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RewardAccrualResponse {
    /// The accrual identity, as a decimal integer string: the `start_key` by
    /// which the history pages.
    #[schema(pattern = "^[0-9]+$", example = "1")]
    pub accrual_id: String,
    /// The account credited, as its 11 zero-padded `ACCT-ID` digits.
    #[schema(pattern = "^[0-9]{11}$", example = "00000000001")]
    pub account_id: String,
    /// The row identity of the transaction that earned the points, as a
    /// decimal integer string. It is not the 16-character `tran_id`: the
    /// accrual keeps this identity after `batch tranbkp` empties the live
    /// transaction master (E-09).
    #[schema(pattern = "^[0-9]+$", example = "1")]
    pub transaction_id: String,
    /// The reward rule that won the match, as a decimal integer string.
    #[schema(pattern = "^[0-9]+$", example = "1")]
    pub rule_id: String,
    /// The points earned, `ceil(amount × reward_rate)`, as a decimal integer
    /// string.
    #[schema(value_type = String, pattern = "^[0-9]+$", example = "100")]
    pub points_earned: String,
    /// The instant of the accrual (the processing clock), in RFC 3339.
    pub created_at: DateTime<Utc>,
}

impl From<&RewardAccrual> for RewardAccrualResponse {
    fn from(accrual: &RewardAccrual) -> Self {
        Self {
            accrual_id: id_string(&accrual.accrual_id),
            account_id: id_string(&accrual.account_id),
            transaction_id: id_string(&accrual.transaction_id),
            rule_id: id_string(&accrual.rule_id),
            points_earned: points_text(accrual.points_earned),
            created_at: accrual.created_at,
        }
    }
}

/// The query of `GET /rewards/history/{account_id}`: `start_key` and
/// `page_size` (request Rewards system; AAP 0.3.5).
///
/// The history lists accruals newest first by `accrual_id` and pages one way
/// only, so it takes no `direction`. Its fields are declared here rather than
/// through a flattened [`PageQuery`](crate::pagination::PageQuery), which
/// would accept and publish a `direction` the history does not have. An
/// out-of-range `page_size` fails
/// to deserialize, so [`ApiQuery`](crate::error::ApiQuery) answers it 400 with
/// code `INVALID`; a malformed `start_key` is answered the same way by
/// [`RewardHistoryQuery::start_accrual_id`].
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RewardHistoryQuery {
    /// The `accrual_id` the page starts at, inclusive: the `next_key` of the
    /// previous page. Absent for the newest page.
    #[serde(default)]
    #[param(pattern = "^[0-9]+$")]
    pub start_key: Option<String>,
    /// Accruals on the page, from 1 to 100. Defaults to 10.
    #[serde(default)]
    #[param(value_type = Option<u16>, minimum = 1, maximum = 100)]
    pub page_size: Option<PageSize>,
}

impl RewardHistoryQuery {
    /// The accrual the page starts at: `None` for an absent `start_key` or
    /// one made only of spaces, which both ask for the newest page.
    ///
    /// # Errors
    ///
    /// A 400 rejection with code `INVALID` ([`ApiError::rejection`]) when
    /// `start_key` is not 1 or more ASCII digits up to `i64::MAX`: no
    /// accrual has such an identity. A sign, an inner or surrounding space and
    /// a decimal point are all rejected. The text names the parameter and the
    /// value, in the form of the other query parameters' rejections.
    pub fn start_accrual_id(&self) -> Result<Option<AccrualId>, ApiError> {
        let Some(raw) = self.start_key.as_deref() else {
            return Ok(None);
        };
        if raw.bytes().all(|byte| byte == b' ') {
            return Ok(None);
        }
        // `i64::from_str` also takes a leading `+` or `-`, which an accrual ID
        // never shows, so the digits are checked first.
        if !raw.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(start_key_rejection(raw));
        }
        raw.parse::<i64>()
            .map(|id| Some(AccrualId::from(id)))
            .map_err(|_| start_key_rejection(raw))
    }
}

/// What a `start_key` must be, as its rejection names it.
struct AccrualKeyExpected;

impl de::Expected for AccrualKeyExpected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "an accrual_id: a decimal integer from 0 to {}",
            i64::MAX
        )
    }
}

/// The 400 answer to a `start_key` that names no accrual: the parameter name
/// and serde's `invalid value` text, as a typed query parameter's
/// deserialization error reads.
fn start_key_rejection(raw: &str) -> ApiError {
    let error = de::value::Error::invalid_value(Unexpected::Str(raw), &AccrualKeyExpected);
    ApiError::rejection(format!("start_key: {error}"))
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::{RewardAccrual, RewardBalance};
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::numeric::{AccountId, AccrualId, RewardPoints, RuleId, TransactionRowId};
    use chrono::{TimeZone, Utc};
    use serde_json::{Value, json};
    use utoipa::{IntoParams, OpenApi, PartialSchema};

    use super::{RewardAccrualResponse, RewardBalanceResponse, RewardHistoryQuery};
    use crate::pagination::{Page, PageSize};

    /// The largest exact JSON number a browser holds is 2^53 − 1; this is
    /// 2^53 + 1, which a JSON number would turn into 2^53.
    const ABOVE_JS_SAFE: i64 = 9_007_199_254_740_993;

    fn points(value: i64) -> RewardPoints {
        RewardPoints::try_from(value).expect("non-negative points")
    }

    fn account(value: u64) -> AccountId {
        AccountId::new(value).expect("an 11-digit account")
    }

    /// The query of `GET /rewards/history/{account_id}?{text}`, as the
    /// endpoint's query extractor reads it.
    fn query(text: &str) -> Result<RewardHistoryQuery, String> {
        let uri: Uri = format!("/rewards/history/00000000001?{text}")
            .parse()
            .expect("a valid URI");
        Query::<RewardHistoryQuery>::try_from_uri(&uri)
            .map(|Query(query)| query)
            .map_err(|rejection| rejection.body_text())
    }

    fn with_start_key(start_key: Option<&str>) -> RewardHistoryQuery {
        RewardHistoryQuery {
            start_key: start_key.map(str::to_owned),
            page_size: None,
        }
    }

    #[test]
    fn balance_points_serialize_as_exact_digit_strings() {
        // (account, earned, redeemed, JSON body)
        let cases = [
            (
                3,
                ABOVE_JS_SAFE,
                0,
                r#"{"account_id":"00000000003","total_points_earned":"9007199254740993","total_points_redeemed":"0","available_points":"9007199254740993"}"#,
            ),
            (
                99_999_999_999,
                i64::MAX,
                0,
                r#"{"account_id":"99999999999","total_points_earned":"9223372036854775807","total_points_redeemed":"0","available_points":"9223372036854775807"}"#,
            ),
            (
                1,
                111,
                11,
                r#"{"account_id":"00000000001","total_points_earned":"111","total_points_redeemed":"11","available_points":"100"}"#,
            ),
        ];
        for (id, earned, redeemed, expected) in cases {
            let balance = RewardBalance::new(account(id), points(earned), points(redeemed))
                .expect("redeemed does not exceed earned");
            let body = RewardBalanceResponse::from(&balance);
            assert_eq!(
                serde_json::to_string(&body).expect("serializes"),
                expected,
                "{id}"
            );
        }
    }

    #[test]
    fn the_zero_balance_serializes_zero_for_every_total() {
        let body = RewardBalanceResponse::from(&RewardBalance::zero(account(2)));
        assert_eq!(
            serde_json::to_value(&body).expect("serializes"),
            json!({
                "account_id": "00000000002",
                "total_points_earned": "0",
                "total_points_redeemed": "0",
                "available_points": "0",
            })
        );
    }

    #[test]
    fn accrual_ids_are_strings_and_created_at_is_rfc3339() {
        let created_at = Utc
            .with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
            .single()
            .expect("a valid UTC instant");
        let accrual = RewardAccrual {
            accrual_id: AccrualId::from(ABOVE_JS_SAFE),
            account_id: account(1),
            transaction_id: TransactionRowId::from(7),
            rule_id: RuleId::from(1),
            points_earned: points(100),
            created_at,
        };
        let body = RewardAccrualResponse::from(&accrual);
        assert_eq!(
            serde_json::to_string(&body).expect("serializes"),
            r#"{"accrual_id":"9007199254740993","account_id":"00000000001","transaction_id":"7","rule_id":"1","points_earned":"100","created_at":"2022-07-01T12:00:00Z"}"#
        );
    }

    #[test]
    fn history_query_reads_start_key_and_a_page_size_of_1_to_100() {
        // (query text, start key, page size)
        let accepted = [
            ("", None, None),
            ("page_size=5", None, Some(5)),
            ("page_size=1", None, Some(1)),
            ("start_key=42&page_size=100", Some("42"), Some(100)),
            ("start_key=", Some(""), None),
            ("direction=backward&page_size=2", None, Some(2)),
        ];
        for (text, start_key, rows) in accepted {
            let parsed = query(text).expect("a valid query");
            assert_eq!(parsed.start_key.as_deref(), start_key, "{text}");
            assert_eq!(parsed.page_size.map(PageSize::get), rows, "{text}");
        }
        for text in [
            "page_size=0",
            "page_size=101",
            "page_size=-1",
            "page_size=x",
        ] {
            let rejection = query(text).expect_err("out of range");
            assert!(
                rejection.contains("expected an integer from 1 to 100"),
                "{text}: {rejection}"
            );
        }
        let empty = RewardHistoryQuery::default();
        assert_eq!(empty.start_key, None);
        assert_eq!(empty.page_size, None);
    }

    #[test]
    fn start_accrual_id_reads_digits_and_treats_blank_as_absent() {
        // (start key, accrual ID)
        let cases = [
            (None, None),
            (Some(""), None),
            (Some("   "), None),
            (Some("42"), Some(42)),
            (Some("0000000042"), Some(42)),
            (Some("0"), Some(0)),
            (Some("9007199254740993"), Some(ABOVE_JS_SAFE)),
            (Some("9223372036854775807"), Some(i64::MAX)),
        ];
        for (start_key, expected) in cases {
            let id = with_start_key(start_key)
                .start_accrual_id()
                .expect("a valid start key");
            assert_eq!(id.map(AccrualId::get), expected, "{start_key:?}");
        }
    }

    #[test]
    fn start_accrual_id_rejects_any_other_text_with_400_invalid() {
        for raw in [
            "abc",
            "-1",
            "+1",
            "4 2",
            " 42",
            "42 ",
            "1.0",
            "0x10",
            "\u{0661}",
            "9223372036854775808",
        ] {
            let error = with_start_key(Some(raw))
                .start_accrual_id()
                .expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{raw:?}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{raw:?}");
            assert!(error.body().fields.is_empty(), "{raw:?}");
            let expected = format!(
                "start_key: invalid value: string {raw:?}, expected an accrual_id: a decimal integer from 0 to 9223372036854775807"
            );
            assert_eq!(error.body().message, expected, "{raw:?}");
        }
    }

    #[test]
    fn openapi_points_and_ids_are_digit_strings() {
        let balance = serde_json::to_value(RewardBalanceResponse::schema()).expect("serializes");
        let accrual = serde_json::to_value(RewardAccrualResponse::schema()).expect("serializes");
        // (schema, property, pattern)
        let cases = [
            (&balance, "account_id", "^[0-9]{11}$"),
            (&balance, "total_points_earned", "^[0-9]+$"),
            (&balance, "total_points_redeemed", "^[0-9]+$"),
            (&balance, "available_points", "^[0-9]+$"),
            (&accrual, "accrual_id", "^[0-9]+$"),
            (&accrual, "account_id", "^[0-9]{11}$"),
            (&accrual, "transaction_id", "^[0-9]+$"),
            (&accrual, "rule_id", "^[0-9]+$"),
            (&accrual, "points_earned", "^[0-9]+$"),
        ];
        for (schema, property, pattern) in cases {
            let field = &schema["properties"][property];
            assert_eq!(field["type"], json!("string"), "{property}");
            assert_eq!(field["pattern"], json!(pattern), "{property}");
        }
        assert_eq!(
            balance["required"],
            json!([
                "account_id",
                "total_points_earned",
                "total_points_redeemed",
                "available_points"
            ])
        );
        assert_eq!(
            accrual["required"],
            json!([
                "accrual_id",
                "account_id",
                "transaction_id",
                "rule_id",
                "points_earned",
                "created_at"
            ])
        );
        let created_at = &accrual["properties"]["created_at"];
        assert_eq!(created_at["type"], json!("string"));
        assert_eq!(created_at["format"], json!("date-time"));
    }

    #[test]
    fn openapi_params_are_the_optional_start_key_and_page_size() {
        let params =
            serde_json::to_value(RewardHistoryQuery::into_params(|| None)).expect("serializes");
        let names: Vec<&str> = params
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["start_key", "page_size"]);
        for param in params.as_array().expect("an array") {
            assert_eq!(param["in"], json!("query"), "{param}");
            assert_eq!(param["required"], json!(false), "{param}");
        }
        assert_eq!(params[0]["schema"]["type"], json!("string"));
        assert_eq!(params[0]["schema"]["pattern"], json!("^[0-9]+$"));
        assert_eq!(params[1]["schema"]["type"], json!("integer"));
        assert_eq!(params[1]["schema"]["minimum"], json!(1));
        assert_eq!(params[1]["schema"]["maximum"], json!(100));
        let text = params.to_string();
        assert!(!text.contains("$ref"), "{text}");
    }

    /// A stand-in of the history operation, declared as the handler declares
    /// its response.
    #[utoipa::path(
        get,
        path = "/rewards/history/{account_id}",
        params(("account_id" = String, Path), RewardHistoryQuery),
        responses((status = 200, body = Page<RewardAccrualResponse>))
    )]
    fn history_probe() {}

    #[derive(OpenApi)]
    #[openapi(paths(history_probe))]
    struct ProbeDoc;

    #[test]
    fn a_history_page_documents_its_accruals_in_the_openapi_document() {
        history_probe();
        let doc = serde_json::to_value(ProbeDoc::openapi()).expect("serializes");
        let operation = &doc["paths"]["/rewards/history/{account_id}"]["get"];
        let names: Vec<&str> = operation["parameters"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|param| param["name"].as_str().expect("a name"))
            .collect();
        assert_eq!(names, ["account_id", "start_key", "page_size"]);

        let content = &operation["responses"]["200"]["content"]["application/json"]["schema"];
        let page = resolve(&doc, content);
        assert_eq!(page["required"], json!(["items", "next_key", "prev_key"]));
        let item = resolve(&doc, &page["properties"]["items"]["items"]);
        assert_eq!(
            item["properties"]["points_earned"]["pattern"],
            json!("^[0-9]+$"),
            "{doc}"
        );
    }

    /// `schema` itself, or the component a `$ref` names.
    fn resolve<'a>(doc: &'a Value, schema: &'a Value) -> &'a Value {
        match schema["$ref"].as_str() {
            Some(reference) => {
                let name = reference
                    .strip_prefix("#/components/schemas/")
                    .expect("a component reference");
                &doc["components"]["schemas"][name]
            }
            None => schema,
        }
    }
}
