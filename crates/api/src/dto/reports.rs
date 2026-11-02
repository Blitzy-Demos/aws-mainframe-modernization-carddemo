//! CR00 transaction reports (`CORPT00C`, map `CORPT00`):
//! `GET /reports/transactions?report_type=&start_date=&end_date=&confirm=`.
//!
//! [`ReportQuery`] carries the `CORPT0A` screen input of one ENTER
//! [app/cpy-bms/CORPT00.CPY:L60-114]: the three selectors `MONTHLY`, `YEARLY`
//! and `CUSTOM` become one `report_type`, the six date parts `SDTYYYY`,
//! `SDTMM`, `SDTDD` and `EDTYYYY`, `EDTMM`, `EDTDD` arrive as one
//! `YYYY-MM-DD` text per date and are split at the hyphens as entered, and
//! `CONFIRM` is `confirm` (E-48). Every edit of `PROCESS-ENTER-KEY` and
//! `SUBMIT-JOB-TO-INTRDR` [app/cbl/CORPT00C.cbl:L208-510] runs in
//! `carddemo_domain::rules::report_request`, which must see the parts exactly
//! as typed; the query only bounds each field's length.
//!
//! [`ReportResponse`] carries the report `CBTRN03C` writes for the confirmed
//! range: the TDQ `JOBS` submission of the `TRANREPT` job becomes a
//! synchronous report sharing the `CBTRN03C` rules with `batch tranrept`
//! (P-CORPT00C-012, E-34). [`ReportRequestField`] lists the BMS fields an error
//! body can name: the `BMS field` values of the `V-CORPT00C` rows of
//! `docs/parity/validation-audit.md`.

use carddemo_domain::rules::Confirm;
use carddemo_domain::rules::report_request::{
    CustomDates, FIELD_CONFIRM, FIELD_EDTDD, FIELD_EDTMM, FIELD_EDTYYYY, FIELD_MONTHLY,
    FIELD_SDTDD, FIELD_SDTMM, FIELD_SDTYYYY, ReportRange, ReportRequestInput, ReportType,
};
use serde::de::IntoDeserializer;
use serde::de::value::StrDeserializer;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{BmsField, reject_invalid, text};
use crate::error::ApiError;

/// The message of a query field longer than its declared length.
///
/// No 3270 field holds more characters than its length, so `CORPT00C` has no
/// path for such input and the validation audit no over-length row: the answer
/// sets no message, and the error body names the field.
const OVER_LENGTH_MESSAGE: &str = "";

/// The `YYYY-MM-DD` text of one date: `SDTYYYYI X(4)`, `SDTMMI X(2)` and
/// `SDTDDI X(2)` with their two hyphens [app/cpy-bms/CORPT00.CPY:L78-90].
const DATE_TEXT_LEN: u64 = 10;

/// The report selector of the map: `MONTHLYI`, `YEARLYI` and `CUSTOMI`, each
/// `X(1)` [app/cpy-bms/CORPT00.CPY:L60-72], become one value naming the
/// selector entered (E-48).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReportTypeParam {
    /// `MONTHLY`: the current calendar month (P-CORPT00C-003).
    Monthly,
    /// `YEARLY`: the current calendar year (P-CORPT00C-004).
    Yearly,
    /// `CUSTOM`: the `start_date` to `end_date` range (P-CORPT00C-005 to
    /// P-CORPT00C-008).
    Custom,
}

impl From<ReportTypeParam> for ReportType {
    fn from(param: ReportTypeParam) -> Self {
        match param {
            ReportTypeParam::Monthly => Self::Monthly,
            ReportTypeParam::Yearly => Self::Yearly,
            ReportTypeParam::Custom => Self::Custom,
        }
    }
}

/// The query of `GET /reports/transactions`: the `CORPT0A` input of one ENTER.
///
/// Every value reaches the rule as entered. Only the declared lengths are
/// checked here ([`ReportQuery::check`]); the empty, value and `CSUTLDTC`
/// edits and the confirmation belong to `report_request::request`, whose
/// first failure answers in the COBOL order (P-CORPT00C-002).
#[derive(Debug, Clone, Default, Deserialize, IntoParams, Validate)]
#[into_params(parameter_in = Query)]
pub struct ReportQuery {
    /// The report to print: `monthly`, `yearly` or `custom`. Absent or blank
    /// is no selection, answered with `Select a report type to print
    /// report...` (V-CORPT00C-001).
    #[serde(default, deserialize_with = "report_type_or_blank")]
    #[param(inline)]
    pub report_type: Option<ReportTypeParam>,
    /// The custom start date, `YYYY-MM-DD`: year, month and day as entered in
    /// `SDTYYYY`, `SDTMM` and `SDTDD`, an empty part standing for a blank
    /// field (V-CORPT00C-002 to V-CORPT00C-004, V-CORPT00C-008 to
    /// V-CORPT00C-010, V-CORPT00C-014). Read only for a custom report.
    #[serde(default)]
    #[param(max_length = 10)]
    #[validate(length(max = DATE_TEXT_LEN))]
    pub start_date: Option<String>,
    /// The custom end date, `YYYY-MM-DD`: `EDTYYYY`, `EDTMM` and `EDTDD`
    /// (V-CORPT00C-005 to V-CORPT00C-007, V-CORPT00C-011 to V-CORPT00C-013,
    /// V-CORPT00C-015). Read only for a custom report.
    #[serde(default)]
    #[param(max_length = 10)]
    #[validate(length(max = DATE_TEXT_LEN))]
    pub end_date: Option<String>,
    /// `CONFIRM X(1)`: `Y` or `y` runs the report, which the SPA confirmation
    /// dialog sends; absent or blank asks for confirmation (V-CORPT00C-016),
    /// `N` or `n` clears the form (V-CORPT00C-017), any other value is
    /// rejected (V-CORPT00C-018).
    #[serde(default)]
    #[param(max_length = 1)]
    #[validate(length(max = 1))]
    pub confirm: Option<String>,
}

impl ReportQuery {
    /// The declared field lengths, checked before the rule runs.
    ///
    /// The date parts are not checked against their map fields here:
    /// `report_request` rejects a part longer than `SDTMMI X(2)`, `SDTDDI
    /// X(2)` or `SDTYYYYI X(4)` with that field's value message once the
    /// COBOL order reaches its edit (E-28, V-CORPT00C-008 to V-CORPT00C-013).
    /// A check here would answer ahead of the earlier edits, the missing
    /// report type and the empty parts included, and without the audited text.
    ///
    /// # Errors
    ///
    /// 400 `Invalid` with no message when `start_date` or `end_date` is longer
    /// than 10 characters or `confirm` longer than 1. A date stands on the
    /// field of its first part, `SDTYYYY` or `EDTYYYY`; `confirm` on
    /// `CONFIRM`. The fields come in the audit order.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, length_field, OVER_LENGTH_MESSAGE))
    }

    /// CORPT00C RECEIVE-TRNRPT-SCREEN — P-CORPT00C-019, E-48
    ///
    /// The rule input: the report type, the six date parts split from the two
    /// dates as entered, and the confirmation classified by [`Confirm::parse`].
    #[must_use]
    pub fn into_input(self) -> ReportRequestInput {
        let (start_year, start_month, start_day) = split_date(self.start_date.as_deref());
        let (end_year, end_month, end_day) = split_date(self.end_date.as_deref());
        ReportRequestInput {
            report_type: self.report_type.map(ReportType::from),
            custom: CustomDates {
                start_month,
                start_day,
                start_year,
                end_month,
                end_day,
                end_year,
            },
            confirm: Confirm::parse(self.confirm.as_deref()),
        }
    }
}

/// The BMS field of a query field that fails its length.
fn length_field(name: &str) -> Option<ReportRequestField> {
    match name {
        "start_date" => Some(ReportRequestField::StartYear),
        "end_date" => Some(ReportRequestField::EndYear),
        "confirm" => Some(ReportRequestField::Confirm),
        _ => None,
    }
}

/// `report_type` as the query carries it, a blank value read as no selection:
/// `MONTHLYI`, `YEARLYI` and `CUSTOMI` all `SPACES` or `LOW-VALUES`
/// (V-CORPT00C-001), as an absent value is.
fn report_type_or_blank<'de, D>(deserializer: D) -> Result<Option<ReportTypeParam>, D::Error>
where
    D: Deserializer<'de>,
{
    let Some(value) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    if is_blank(&value) {
        return Ok(None);
    }
    let value: StrDeserializer<'_, D::Error> = value.as_str().into_deserializer();
    ReportTypeParam::deserialize(value).map(Some)
}

/// The COBOL condition `= SPACES OR LOW-VALUES` on entered text: empty, every
/// character a space, or every character `NUL`.
fn is_blank(value: &str) -> bool {
    value.bytes().all(|byte| byte == b' ') || value.bytes().all(|byte| byte == 0)
}

/// One `YYYY-MM-DD` date split at its first two hyphens into year, month and
/// day, each exactly as entered: nothing trimmed, padded or parsed, and a
/// missing part empty, so `"2022-1"` gives `("2022", "1", "")` (E-48). Text
/// after a third hyphen stays in the day.
fn split_date(value: Option<&str>) -> (String, String, String) {
    let mut parts = value.unwrap_or_default().splitn(3, '-').map(str::to_owned);
    let year = parts.next().unwrap_or_default();
    let month = parts.next().unwrap_or_default();
    let day = parts.next().unwrap_or_default();
    (year, month, day)
}

/// The answer to a confirmed request: the report `CBTRN03C` writes over the
/// range, and the `CORPT00C` text of the submission.
///
/// Only a run answers 200. Every other outcome of the request sets
/// `WS-ERR-FLG`, the confirmation prompt and the `N` that clears the form
/// included (V-CORPT00C-016, V-CORPT00C-017), so it is an error body, never a
/// message-only report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ReportResponse {
    /// `WS-REPORT-NAME`: `Monthly`, `Yearly` or `Custom`.
    pub report_name: String,
    /// `WS-START-DATE`, `YYYY-MM-DD`: the first processing date reported
    /// (P-CORPT00C-017).
    pub start_date: String,
    /// `WS-END-DATE`, `YYYY-MM-DD`: the last processing date reported.
    pub end_date: String,
    /// The `CVTRA07Y` report records in the order `CBTRN03C` writes them:
    /// headers, detail lines, and page, account and grand totals
    /// (P-CBTRN03C-003 to P-CBTRN03C-013).
    ///
    /// Each record is fixed width and kept untrimmed, trailing spaces
    /// included: the SPA shows the lines in a monospace block, and trimming
    /// them would break parity with the `TRANREPT` records of
    /// `batch tranrept`.
    pub lines: Vec<String>,
    /// `WS-MESSAGE`: `<name> report submitted for printing ...`
    /// (P-CORPT00C-014).
    pub message: String,
}

impl ReportResponse {
    /// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-012, P-CORPT00C-014,
    /// P-CORPT00C-017
    ///
    /// The response of a run over `range`: its name and dates, the report
    /// `lines` as `CBTRN03C` wrote them, and the submission `message`.
    #[must_use]
    pub fn generated(range: &ReportRange, lines: Vec<String>, message: String) -> Self {
        Self {
            report_name: range.report_name.to_owned(),
            start_date: text(&range.start),
            end_date: text(&range.end),
            lines,
            message,
        }
    }
}

/// A BMS field of `CORPT0A` that a CR00 error body names: the distinct
/// `BMS field` values of the `V-CORPT00C` rows. `YEARLY` and `CUSTOM` are no
/// error's field: a missing report type and the confirmation `N` place the
/// cursor on `MONTHLY` (V-CORPT00C-001, V-CORPT00C-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum ReportRequestField {
    /// `MONTHLYI X(1)`: no report type selected (V-CORPT00C-001), and the form
    /// the confirmation `N` clears (V-CORPT00C-017).
    #[serde(rename = "MONTHLY")]
    Monthly,
    /// `SDTMMI X(2)`: the start month (V-CORPT00C-002, V-CORPT00C-008), and
    /// the cursor of the start date check (V-CORPT00C-014).
    #[serde(rename = "SDTMM")]
    StartMonth,
    /// `SDTDDI X(2)`: the start day (V-CORPT00C-003, V-CORPT00C-009).
    #[serde(rename = "SDTDD")]
    StartDay,
    /// `SDTYYYYI X(4)`: the start year (V-CORPT00C-004, V-CORPT00C-010), and
    /// a `start_date` longer than 10 characters.
    #[serde(rename = "SDTYYYY")]
    StartYear,
    /// `EDTMMI X(2)`: the end month (V-CORPT00C-005, V-CORPT00C-011), and the
    /// cursor of the end date check (V-CORPT00C-015).
    #[serde(rename = "EDTMM")]
    EndMonth,
    /// `EDTDDI X(2)`: the end day (V-CORPT00C-006, V-CORPT00C-012).
    #[serde(rename = "EDTDD")]
    EndDay,
    /// `EDTYYYYI X(4)`: the end year (V-CORPT00C-007, V-CORPT00C-013), and an
    /// `end_date` longer than 10 characters.
    #[serde(rename = "EDTYYYY")]
    EndYear,
    /// `CONFIRMI X(1)`: the confirmation prompt and an invalid confirmation
    /// (V-CORPT00C-016, V-CORPT00C-018), and a `confirm` longer than 1.
    #[serde(rename = "CONFIRM")]
    Confirm,
}

impl ReportRequestField {
    /// The BMS symbolic field name, the `fields[].field` value of the error
    /// body: the field name the rule sets.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Monthly => FIELD_MONTHLY,
            Self::StartMonth => FIELD_SDTMM,
            Self::StartDay => FIELD_SDTDD,
            Self::StartYear => FIELD_SDTYYYY,
            Self::EndMonth => FIELD_EDTMM,
            Self::EndDay => FIELD_EDTDD,
            Self::EndYear => FIELD_EDTYYYY,
            Self::Confirm => FIELD_CONFIRM,
        }
    }

    /// The `Order` of the field in the audit: `MONTHLY` 1 (V-CORPT00C-001);
    /// each date part the order of its value edit, 3 to 8 (V-CORPT00C-008 to
    /// V-CORPT00C-013), which is also the screen order of the empty checks
    /// that share order 2; `CONFIRM` 11 (V-CORPT00C-016).
    const fn audit_order(self) -> u16 {
        match self {
            Self::Monthly => 1,
            Self::StartMonth => 3,
            Self::StartDay => 4,
            Self::StartYear => 5,
            Self::EndMonth => 6,
            Self::EndDay => 7,
            Self::EndYear => 8,
            Self::Confirm => 11,
        }
    }
}

impl BmsField for ReportRequestField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    fn rank(self) -> u16 {
        self.audit_order()
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::Alnum;
    use carddemo_domain::rules::Confirm;
    use carddemo_domain::rules::report_request::{
        self, CustomDates, ReportRange, ReportRequestInput, ReportRequestOutcome, ReportType,
    };
    use chrono::NaiveDate;
    use serde_json::{Value, json};
    use utoipa::IntoParams;
    use utoipa::openapi::path::ParameterIn;

    use super::{
        BmsField, ReportQuery, ReportRequestField, ReportResponse, ReportTypeParam, split_date,
    };
    use crate::error::ApiError;

    const ALL_FIELDS: [ReportRequestField; 8] = [
        ReportRequestField::Monthly,
        ReportRequestField::StartMonth,
        ReportRequestField::StartDay,
        ReportRequestField::StartYear,
        ReportRequestField::EndMonth,
        ReportRequestField::EndDay,
        ReportRequestField::EndYear,
        ReportRequestField::Confirm,
    ];

    /// The query `GET /reports/transactions?<text>` gives through axum's
    /// extractor, which reads it with `serde_urlencoded`.
    fn query(text: &str) -> Result<ReportQuery, String> {
        let uri: Uri = format!("/reports/transactions?{text}")
            .parse()
            .map_err(|error| format!("{error}"))?;
        Query::<ReportQuery>::try_from_uri(&uri)
            .map(|Query(query)| query)
            .map_err(|rejection| rejection.body_text())
    }

    fn owned(year: &str, month: &str, day: &str) -> (String, String, String) {
        (year.to_owned(), month.to_owned(), day.to_owned())
    }

    /// The JSON of the error body.
    fn body(error: &ApiError) -> Value {
        serde_json::to_value(error.body()).expect("the error body serializes")
    }

    #[test]
    fn split_date_keeps_each_part_as_entered() {
        let cases = [
            (Some("2022-01-01"), owned("2022", "01", "01")),
            (Some("2022-1"), owned("2022", "1", "")),
            (None, owned("", "", "")),
            (Some(""), owned("", "", "")),
            (Some("--"), owned("", "", "")),
            (Some("2022--01"), owned("2022", "", "01")),
            (Some("-01-01"), owned("", "01", "01")),
            (Some("2022-01-"), owned("2022", "01", "")),
            (Some("2022"), owned("2022", "", "")),
            (Some(" 2022- 1-1 "), owned(" 2022", " 1", "1 ")),
            (Some("2022-01-01-5"), owned("2022", "01", "01-5")),
            (Some("20A2-1.5-$3"), owned("20A2", "1.5", "$3")),
        ];
        for (value, expected) in cases {
            assert_eq!(split_date(value), expected, "{value:?}");
        }
    }

    #[test]
    fn report_type_reads_the_lowercase_names_and_blank_as_no_selection() {
        let cases = [
            ("report_type=monthly", Some(ReportTypeParam::Monthly)),
            ("report_type=yearly", Some(ReportTypeParam::Yearly)),
            ("report_type=custom", Some(ReportTypeParam::Custom)),
            ("", None),
            ("start_date=2022-01-01", None),
            ("report_type=", None),
            ("report_type=%20%20", None),
            ("report_type=%00", None),
        ];
        for (text, expected) in cases {
            let parsed = query(text).unwrap_or_else(|error| panic!("{text}: {error}"));
            assert_eq!(parsed.report_type, expected, "{text}");
        }
        for text in [
            "report_type=Custom",
            "report_type=weekly",
            "report_type=%20custom",
        ] {
            assert!(query(text).is_err(), "{text} must be rejected");
        }
    }

    #[test]
    fn query_keeps_every_value_as_sent() {
        let parsed =
            query("report_type=custom&start_date=2022-01-01&end_date=%202022-07-6&confirm=y")
                .expect("a complete custom query");
        assert_eq!(parsed.report_type, Some(ReportTypeParam::Custom));
        assert_eq!(parsed.start_date.as_deref(), Some("2022-01-01"));
        assert_eq!(parsed.end_date.as_deref(), Some(" 2022-07-6"));
        assert_eq!(parsed.confirm.as_deref(), Some("y"));

        let empty = query("start_date=&end_date=&confirm=").expect("empty values");
        assert_eq!(empty.start_date.as_deref(), Some(""));
        assert_eq!(empty.end_date.as_deref(), Some(""));
        assert_eq!(empty.confirm.as_deref(), Some(""));
    }

    #[test]
    fn report_type_param_maps_to_the_rule_selector() {
        let cases = [
            (ReportTypeParam::Monthly, ReportType::Monthly, "\"monthly\""),
            (ReportTypeParam::Yearly, ReportType::Yearly, "\"yearly\""),
            (ReportTypeParam::Custom, ReportType::Custom, "\"custom\""),
        ];
        for (param, selector, json) in cases {
            assert_eq!(ReportType::from(param), selector);
            assert_eq!(serde_json::to_string(&param).expect("serializes"), json);
        }
    }

    #[test]
    fn into_input_splits_the_dates_and_classifies_the_confirmation() {
        let input = ReportQuery {
            report_type: Some(ReportTypeParam::Custom),
            start_date: Some("2022-01-01".to_owned()),
            end_date: Some("2022-7".to_owned()),
            confirm: Some("Y".to_owned()),
        }
        .into_input();
        assert_eq!(
            input,
            ReportRequestInput {
                report_type: Some(ReportType::Custom),
                custom: CustomDates {
                    start_month: "01".to_owned(),
                    start_day: "01".to_owned(),
                    start_year: "2022".to_owned(),
                    end_month: "7".to_owned(),
                    end_day: String::new(),
                    end_year: "2022".to_owned(),
                },
                confirm: Confirm::Yes,
            }
        );

        let empty = ReportQuery::default().into_input();
        assert_eq!(empty.report_type, None);
        assert_eq!(empty.custom, CustomDates::default());
        assert_eq!(empty.confirm, Confirm::Blank);

        for (confirm, expected) in [
            ("N", Confirm::No),
            (" ", Confirm::Blank),
            ("X", Confirm::Other("X".to_owned())),
        ] {
            let input = ReportQuery {
                confirm: Some(confirm.to_owned()),
                ..ReportQuery::default()
            }
            .into_input();
            assert_eq!(input.confirm, expected, "{confirm:?}");
        }
    }

    #[test]
    fn check_rejects_only_the_declared_lengths_in_audit_order() {
        let date_over = "20221-01-01";
        let cases = [
            (Some(date_over), None, None, vec!["SDTYYYY"]),
            (None, Some("2022-07-066"), None, vec!["EDTYYYY"]),
            (None, None, Some("YY"), vec!["CONFIRM"]),
            (
                Some(date_over),
                Some(date_over),
                Some("YY"),
                vec!["SDTYYYY", "EDTYYYY", "CONFIRM"],
            ),
        ];
        for (start_date, end_date, confirm, fields) in cases {
            let query = ReportQuery {
                report_type: Some(ReportTypeParam::Custom),
                start_date: start_date.map(str::to_owned),
                end_date: end_date.map(str::to_owned),
                confirm: confirm.map(str::to_owned),
            };
            let error = query.check().expect_err("a field is too long");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{fields:?}");
            let fields: Vec<Value> = fields
                .into_iter()
                .map(|field| json!({"field": field, "message": ""}))
                .collect();
            assert_eq!(
                body(&error),
                json!({"code": ErrorKind::Invalid.code(), "message": "", "fields": fields})
            );
        }

        // At their limits the fields pass, a date part longer than its map
        // field included: the rule answers it in the COBOL order.
        for (start_date, end_date, confirm) in [
            (Some("2022-01-01"), Some("2022-07-06"), Some("Y")),
            (Some("2022-001-1"), Some("1-2-345678"), Some(" ")),
            (Some(""), Some(""), Some("")),
            (None, None, None),
        ] {
            let query = ReportQuery {
                report_type: None,
                start_date: start_date.map(str::to_owned),
                end_date: end_date.map(str::to_owned),
                confirm: confirm.map(str::to_owned),
            };
            assert!(query.check().is_ok(), "{query:?}");
        }
    }

    /// The rule's answer to the query `text`, on 2022-07-01.
    fn request(text: &str) -> Result<ReportRequestOutcome, DomainError> {
        let parsed = query(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        assert!(parsed.check().is_ok(), "{text} passes its lengths");
        let today = NaiveDate::from_ymd_opt(2022, 7, 1).expect("a calendar date");
        report_request::request(&parsed.into_input(), today)
    }

    /// The message and field names of the rule's rejection of the query
    /// `text`.
    fn rejection(text: &str) -> (String, Vec<&'static str>) {
        let error = request(text).expect_err("the rule rejects the query");
        let fields = error.fields.iter().map(|entry| entry.field).collect();
        (error.message, fields)
    }

    #[test]
    fn the_rule_sees_the_parts_as_entered() {
        let cases = [
            // An over-long part is the rule's E-28 rejection, after the
            // earlier edits: the missing report type answers first.
            (
                "start_date=2022-001-1&end_date=2022-07-06&confirm=Y",
                messages::SELECT_A_REPORT_TYPE_TO_PRINT_REPORT,
                "MONTHLY",
            ),
            (
                "report_type=custom&start_date=2022-001-1&end_date=2022-07-06&confirm=Y",
                messages::START_DATE_NOT_A_VALID_MONTH,
                "SDTMM",
            ),
            (
                "report_type=custom&start_date=-01-01&end_date=2022-07-06&confirm=Y",
                messages::START_DATE_YEAR_CAN_NOT_BE_EMPTY,
                "SDTYYYY",
            ),
            (
                "report_type=custom&start_date=2022-01-01&end_date=2022-07-32&confirm=Y",
                messages::END_DATE_NOT_A_VALID_DAY,
                "EDTDD",
            ),
        ];
        for (text, message, field) in cases {
            assert_eq!(rejection(text), (message.to_owned(), vec![field]), "{text}");
        }
    }

    #[test]
    fn every_field_the_rule_names_is_a_report_request_field() {
        let names: Vec<&str> = ALL_FIELDS.iter().map(|field| field.as_str()).collect();
        for text in [
            "",
            "report_type=custom&start_date=2022--01&end_date=2022-07-06",
            "report_type=custom&start_date=2022-01-&end_date=2022-07-06",
            "report_type=custom&start_date=-01-01&end_date=2022-07-06",
            "report_type=custom&start_date=2022-01-01&end_date=2022--06",
            "report_type=custom&start_date=2022-01-01&end_date=2022-07-",
            "report_type=custom&start_date=2022-01-01&end_date=-07-06",
            "report_type=custom&start_date=2022-13-01&end_date=2022-07-06",
            "report_type=custom&start_date=2022-01-32&end_date=2022-07-06",
            "report_type=custom&start_date=20A2-01-01&end_date=2022-07-06",
            "report_type=custom&start_date=2022-01-01&end_date=2022-13-06",
            "report_type=custom&start_date=2022-01-01&end_date=2022-07-32",
            "report_type=custom&start_date=2022-01-01&end_date=20A2-07-06",
            "report_type=custom&start_date=2022-02-30&end_date=2022-07-06",
            "report_type=custom&start_date=2022-01-01&end_date=2022-04-31",
            "report_type=monthly",
            "report_type=yearly&confirm=N",
            "report_type=yearly&confirm=X",
        ] {
            let (_, fields) = rejection(text);
            assert!(!fields.is_empty(), "{text}: the rejection names a field");
            for field in fields {
                assert!(names.contains(&field), "{text}: {field} is not listed");
            }
        }
    }

    #[test]
    fn a_confirmed_custom_request_reports_its_range() {
        let outcome =
            request("report_type=custom&start_date=2022-01-01&end_date=2022-07-06&confirm=Y")
                .expect("every edit passes");
        let ReportRequestOutcome::Submitted { range, message } = outcome;
        assert_eq!(
            range,
            ReportRange {
                report_name: messages::REPORT_NAME_CUSTOM,
                start: Alnum::move_from("2022-01-01"),
                end: Alnum::move_from("2022-07-06"),
            }
        );
        assert_eq!(
            message,
            messages::report_submitted_message(messages::REPORT_NAME_CUSTOM)
        );
    }

    #[test]
    fn report_response_keeps_the_lines_untrimmed() {
        let range = ReportRange {
            report_name: messages::REPORT_NAME_MONTHLY,
            start: Alnum::move_from("2022-07-01"),
            end: Alnum::move_from("2022-07-31"),
        };
        let lines = vec![
            format!("{:<20}{}", "DALYREPT", " ".repeat(113)),
            String::new(),
            "   ".to_owned(),
            format!("{:>133}", "+         10.00"),
        ];
        let message = messages::report_submitted_message(messages::REPORT_NAME_MONTHLY);
        let response = ReportResponse::generated(&range, lines.clone(), message.clone());
        assert_eq!(
            response,
            ReportResponse {
                report_name: "Monthly".to_owned(),
                start_date: "2022-07-01".to_owned(),
                end_date: "2022-07-31".to_owned(),
                lines: lines.clone(),
                message: message.clone(),
            }
        );
        let wire = serde_json::to_value(&response).expect("serializes");
        assert_eq!(
            wire,
            json!({
                "report_name": "Monthly",
                "start_date": "2022-07-01",
                "end_date": "2022-07-31",
                "lines": lines,
                "message": message,
            })
        );
        let lengths: Vec<usize> = wire["lines"]
            .as_array()
            .expect("lines is an array")
            .iter()
            .map(|line| line.as_str().map_or(0, str::len))
            .collect();
        assert_eq!(lengths, [133, 0, 3, 133]);
        let back: ReportResponse = serde_json::from_value(wire).expect("deserializes");
        assert_eq!(back, response);
    }

    #[test]
    fn report_request_field_names_the_bms_fields_in_audit_order() {
        let expected = [
            "MONTHLY", "SDTMM", "SDTDD", "SDTYYYY", "EDTMM", "EDTDD", "EDTYYYY", "CONFIRM",
        ];
        for (field, name) in ALL_FIELDS.into_iter().zip(expected) {
            assert_eq!(field.as_str(), name);
            assert_eq!(BmsField::as_str(field), name);
            assert_eq!(
                serde_json::to_value(field).expect("serializes"),
                json!(name)
            );
            let back: ReportRequestField =
                serde_json::from_value(json!(name)).expect("deserializes");
            assert_eq!(back, field);
        }
        let ranks: Vec<u16> = ALL_FIELDS.into_iter().map(BmsField::rank).collect();
        assert_eq!(ranks, [1, 3, 4, 5, 6, 7, 8, 11]);
    }

    #[test]
    fn query_parameters_document_the_lengths_and_report_types() {
        let params = serde_json::to_value(ReportQuery::into_params(|| Some(ParameterIn::Query)))
            .expect("the parameters serialize");
        let params = params.as_array().expect("a parameter list");
        let names: Vec<&str> = params
            .iter()
            .filter_map(|param| param["name"].as_str())
            .collect();
        assert_eq!(names, ["report_type", "start_date", "end_date", "confirm"]);
        for param in params {
            assert_eq!(param["in"], "query", "{param}");
            assert_ne!(param["required"], true, "{param}");
        }
        let text = |index: usize| params[index]["schema"].to_string();
        for value in ["monthly", "yearly", "custom"] {
            assert!(text(0).contains(&format!("\"{value}\"")), "{}", text(0));
        }
        assert!(
            !text(0).contains("$ref"),
            "report_type is inline: {}",
            text(0)
        );
        assert_eq!(params[1]["schema"]["maxLength"], 10);
        assert_eq!(params[2]["schema"]["maxLength"], 10);
        assert_eq!(params[3]["schema"]["maxLength"], 1);
    }
}
